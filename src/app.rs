//! The installer's window. Downloads, installs and the obs-browser step run
//! on a thread of their own, one at a time, so the window never freezes.

use crate::github::{self, Latest};
use crate::obs::{self, BrowserState};
use crate::plugins::{self, Installed, PLUGINS};
use eframe::egui::{self, Align, Color32, Layout, RichText, Vec2};
use semver::Version;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const INSTALLER_ID: &str = "theater-installer";
const ZIP_ENDING: &str = "-windows-x64.zip";
const RED: Color32 = Color32::from_rgb(240, 96, 88);
const GREEN: Color32 = Color32::from_rgb(110, 200, 120);
const AMBER: Color32 = Color32::from_rgb(240, 190, 80);

#[derive(Default)]
struct Shared {
    /// None while GitHub is being asked
    latest: Option<Result<HashMap<String, Latest>, String>>,
    /// What's being done, and how far along when that's known
    busy: Option<(String, Option<f32>)>,
    /// (is an error, message)
    log: Vec<(bool, String)>,
    /// Something changed on disk: look again
    changed: bool,
}

/// What the work thread can tell the window
struct Job {
    shared: Arc<Mutex<Shared>>,
    ctx: egui::Context,
}

impl Job {
    fn progress(&self, what: &str, done: Option<f32>) {
        self.shared.lock().unwrap().busy = Some((what.to_string(), done));
        self.ctx.request_repaint();
    }

    fn log(&self, message: String) {
        self.shared.lock().unwrap().log.push((false, message));
        self.ctx.request_repaint();
    }
}

enum Action {
    Check,
    Install(Vec<usize>),
    AskRemove(Option<usize>),
    Remove(usize),
    Browser { disable: bool },
    Open(String),
    ChooseObs,
    /// Forget the folder you chose and use the one found
    FindObs,
}

pub struct App {
    plugins_dir: PathBuf,
    obs_dir: Option<PathBuf>,
    obs_version: Option<String>,
    /// The OBS folder is the one you chose, not the one found
    obs_chosen: bool,
    obs_running: bool,
    looked_at: Instant,
    installed: Vec<Installed>,
    browser: Option<BrowserState>,
    shared: Arc<Mutex<Shared>>,
    icons: Vec<egui::TextureHandle>,
    installer_icon: egui::TextureHandle,
    /// The obs-browser disclaimer's checkbox
    understood: bool,
    confirm_remove: Option<usize>,
}

pub fn run() -> eframe::Result {
    let icon = image::load_from_memory(include_bytes!("../assets/installer.png")).expect("built-in icon").to_rgba8();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Theater Installer")
            .with_inner_size([780.0, 720.0])
            .with_min_inner_size([600.0, 520.0])
            .with_icon(egui::IconData { width: icon.width(), height: icon.height(), rgba: icon.into_raw() }),
        ..Default::default()
    };
    eframe::run_native("Theater Installer", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

/// A built-in PNG as a texture, made small first so it stays smooth on screen
fn texture(ctx: &egui::Context, name: &str, png: &[u8]) -> egui::TextureHandle {
    let img = image::load_from_memory(png).expect("built-in icon").to_rgba8();
    let img = image::imageops::resize(&img, 128, 128, image::imageops::FilterType::Lanczos3);
    let size = [img.width() as usize, img.height() as usize];
    ctx.load_texture(name, egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), egui::TextureOptions::LINEAR)
}

fn version_text(installed: &Installed) -> String {
    match installed {
        Installed::No => "Not installed".into(),
        Installed::Unknown => "Installed (version unknown)".into(),
        Installed::Version(v) => format!("Installed: {v}"),
    }
}

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        let ctx = &cc.egui_ctx;
        ctx.set_visuals(egui::Visuals::dark());
        ctx.style_mut(|style| {
            use egui::{FontId, TextStyle};
            style.text_styles.insert(TextStyle::Body, FontId::proportional(14.5));
            style.text_styles.insert(TextStyle::Button, FontId::proportional(14.5));
            style.text_styles.insert(TextStyle::Heading, FontId::proportional(24.0));
            style.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        });
        let obs_dir = obs::install_dir();
        let mut app = App {
            plugins_dir: plugins::plugins_dir(),
            obs_version: obs_dir.as_deref().and_then(obs::version),
            obs_chosen: obs::chosen_dir().is_some(),
            obs_dir,
            obs_running: false,
            looked_at: Instant::now(),
            installed: Vec::new(),
            browser: None,
            shared: Arc::default(),
            icons: PLUGINS.iter().map(|p| texture(ctx, p.id, p.icon)).collect(),
            installer_icon: texture(ctx, INSTALLER_ID, include_bytes!("../assets/installer.png")),
            understood: false,
            confirm_remove: None,
        };
        app.look();
        app.check_github(ctx);
        app
    }

    /// What's on this PC now
    fn look(&mut self) {
        self.obs_running = obs::is_running();
        self.installed = PLUGINS.iter().map(|p| plugins::installed(&self.plugins_dir, p.id)).collect();
        self.browser = self.obs_dir.as_deref().map(obs::browser_state);
        self.looked_at = Instant::now();
    }

    /// Switches to another OBS folder (or none)
    fn use_obs(&mut self, dir: Option<PathBuf>, chosen: bool) {
        self.obs_version = dir.as_deref().and_then(obs::version);
        self.obs_dir = dir;
        self.obs_chosen = chosen;
        self.understood = false;
        self.look();
    }

    /// A line in the log
    fn note(&self, error: bool, message: String) {
        self.shared.lock().unwrap().log.push((error, message));
    }

    fn check_github(&self, ctx: &egui::Context) {
        self.shared.lock().unwrap().latest = None;
        let (shared, ctx) = (self.shared.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut wanted: Vec<(&str, &str)> = PLUGINS.iter().map(|p| (p.id, ZIP_ENDING)).collect();
            wanted.push((INSTALLER_ID, ".exe"));
            let result = github::latest(&wanted);
            shared.lock().unwrap().latest = Some(result);
            ctx.request_repaint();
        });
    }

    /// Runs `work` on its own thread unless something else is running. Its
    /// Ok message (if not empty) or its error goes in the log.
    fn spawn(&self, ctx: &egui::Context, what: String, work: impl FnOnce(&Job) -> Result<String, String> + Send + 'static) {
        {
            let mut shared = self.shared.lock().unwrap();
            if shared.busy.is_some() {
                return;
            }
            shared.busy = Some((what, None));
        }
        let job = Job { shared: self.shared.clone(), ctx: ctx.clone() };
        std::thread::spawn(move || {
            let result = work(&job);
            let mut shared = job.shared.lock().unwrap();
            match result {
                Ok(message) if message.is_empty() => {}
                Ok(message) => shared.log.push((false, message)),
                Err(message) => shared.log.push((true, message)),
            }
            shared.busy = None;
            shared.changed = true;
            drop(shared);
            job.ctx.request_repaint();
        });
    }

    fn install(&self, ctx: &egui::Context, which: Vec<usize>, latest: &HashMap<String, Latest>) {
        let items: Vec<(usize, Latest)> = which.into_iter().filter_map(|i| latest.get(PLUGINS[i].id).map(|l| (i, l.clone()))).collect();
        let Some(&(first, _)) = items.first() else { return };
        let what = if items.len() == 1 { format!("Installing {}", PLUGINS[first].name) } else { "Updating the plugins".to_string() };
        let dir = self.plugins_dir.clone();
        self.spawn(ctx, what, move |job| {
            let temp = std::env::temp_dir().join("Theater-Installer");
            std::fs::create_dir_all(&temp).map_err(|e| format!("Couldn't make a folder for the downloads ({}): {e}", temp.display()))?;
            for (i, latest) in &items {
                let plugin = &PLUGINS[*i];
                if obs::is_running() {
                    return Err("OBS was opened. Close it and try again.".into());
                }
                let zip = temp.join(&latest.asset.name);
                let downloading = format!("Downloading {} {}", plugin.name, latest.version);
                github::download(&latest.asset, &zip, &|done| job.progress(&downloading, Some(done)))?;
                job.progress(&format!("Installing {} {}", plugin.name, latest.version), None);
                let installed = plugins::install_zip(&dir, plugin.id, &zip, &latest.version);
                let _ = std::fs::remove_file(&zip);
                installed?;
                job.log(format!("{} {} is installed.", plugin.name, latest.version));
            }
            Ok(if items.len() == 1 { "Start OBS to use it." } else { "Start OBS to use them." }.to_string())
        });
    }

    fn remove(&self, ctx: &egui::Context, i: usize) {
        let dir = self.plugins_dir.clone();
        let plugin = &PLUGINS[i];
        self.spawn(ctx, format!("Removing {}", plugin.name), move |_| {
            if obs::is_running() {
                return Err("OBS was opened. Close it and try again.".into());
            }
            plugins::remove(&dir, plugin.id)?;
            Ok(format!("{} is removed. Its settings stay in {}.", plugin.name, plugins::settings_dir(plugin.id).display()))
        });
    }

    fn set_browser(&self, ctx: &egui::Context, disable: bool) {
        let Some(obs_dir) = self.obs_dir.clone() else { return };
        let what = if disable { "Disabling obs-browser (Windows may ask for permission)" } else { "Restoring obs-browser (Windows may ask for permission)" };
        self.spawn(ctx, what.to_string(), move |_| {
            if obs::is_running() {
                return Err("OBS was opened. Close it and try again.".into());
            }
            obs::set_browser_as_needed(&obs_dir, disable)?;
            Ok(if disable {
                "obs-browser is disabled: its files are renamed to .old. Restore puts them back."
            } else {
                "obs-browser is restored: Browser Sources, Custom Browser Docks and account connections work again."
            }
            .to_string())
        });
    }

    fn header(&self, ui: &mut egui::Ui, latest: &Option<Result<HashMap<String, Latest>, String>>, actions: &mut Vec<Action>) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(&self.installer_icon).fit_to_exact_size(Vec2::splat(52.0)));
            ui.vertical(|ui| {
                ui.heading(RichText::new("Theater Installer").strong());
                ui.label(RichText::new(format!("Installs and updates the Theater plugins for OBS Studio · version {}", env!("CARGO_PKG_VERSION"))).weak());
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let checking = latest.is_none();
                if ui.add_enabled(!checking, egui::Button::new("🔄 Check again")).on_hover_text("Ask GitHub for the newest versions").clicked() {
                    actions.push(Action::Check);
                }
                if checking {
                    ui.spinner();
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            match &self.obs_dir {
                Some(dir) => {
                    let version = self.obs_version.as_deref().unwrap_or("(version unknown)");
                    let chosen = if self.obs_chosen { " (the folder you chose)" } else { "" };
                    ui.label(RichText::new(format!("OBS Studio {version} in {}{chosen}", dir.display())).weak());
                    if ui.small_button("Open").on_hover_text("Open OBS's folder in Explorer").clicked() {
                        actions.push(Action::Open(dir.display().to_string()));
                    }
                }
                None => {
                    ui.colored_label(AMBER, "⚠ OBS Studio wasn't found on this PC. If it's there, choose its folder. The plugins can still be installed, for when it is.");
                }
            }
            if ui
                .small_button("📁 Choose OBS folder…")
                .on_hover_text("When the installer finds the wrong OBS, or none (Steam, portable, another drive): pick the folder OBS is in, the one with bin, data and obs-plugins in it")
                .clicked()
            {
                actions.push(Action::ChooseObs);
            }
            if self.obs_chosen && ui.small_button("Find it by itself").on_hover_text("Forget the folder you chose").clicked() {
                actions.push(Action::FindObs);
            }
        });
        let major = self.obs_version.as_deref().and_then(|v| v.split('.').next()?.parse::<u32>().ok());
        if major.is_some_and(|m| m < 30) {
            ui.colored_label(AMBER, "⚠ Theater needs OBS Studio 30 or newer. Update OBS first.");
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("Plugins go in {}", self.plugins_dir.display())).weak());
            if self.plugins_dir.is_dir() && ui.small_button("Open").on_hover_text("Open the plugins folder in Explorer").clicked() {
                actions.push(Action::Open(self.plugins_dir.display().to_string()));
            }
        });
        if self.obs_running {
            notice(ui, AMBER, "⚠ OBS is open. Close it to install, update or remove; this window notices by itself.");
        }
        match latest {
            Some(Err(e)) => notice(ui, RED, &format!("⚠ {e}")),
            Some(Ok(found)) => {
                if let Some(newer) = found.get(INSTALLER_ID).filter(|l| l.version > Version::parse(env!("CARGO_PKG_VERSION")).unwrap()) {
                    ui.horizontal(|ui| {
                        ui.colored_label(AMBER, format!("A newer Theater Installer is out: {}.", newer.version));
                        if ui.button("Download it").clicked() {
                            actions.push(Action::Open(newer.page.clone()));
                        }
                    });
                }
            }
            None => {}
        }
        ui.add_space(8.0);
    }

    fn plugin_rows(&mut self, ui: &mut egui::Ui, latest: &Option<Result<HashMap<String, Latest>, String>>, blocked: bool, actions: &mut Vec<Action>) {
        let found = match latest {
            Some(Ok(found)) => Some(found),
            _ => None,
        };
        let newest = |i: usize| found.and_then(|f| f.get(PLUGINS[i].id));
        let outdated = |i: usize, installed: &Installed| match (installed, newest(i)) {
            (Installed::Unknown, Some(_)) => true,
            (Installed::Version(v), Some(l)) => l.version > *v,
            _ => false,
        };
        let updates: Vec<usize> = (0..PLUGINS.len()).filter(|&i| outdated(i, &self.installed[i])).collect();

        ui.horizontal(|ui| {
            ui.label(RichText::new("Plugins").size(18.0).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if updates.len() > 1 && ui.add_enabled(!blocked, egui::Button::new(format!("Update all ({})", updates.len()))).clicked() {
                    actions.push(Action::Install(updates.clone()));
                }
            });
        });

        for (i, plugin) in PLUGINS.iter().enumerate() {
            let installed = &self.installed[i];
            egui::Frame::group(ui.style()).inner_margin(12.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    ui.add(egui::Image::new(&self.icons[i]).fit_to_exact_size(Vec2::splat(56.0)));
                    let text_width = (ui.available_width() - 190.0).max(220.0);
                    ui.vertical(|ui| {
                        ui.set_width(text_width);
                        ui.label(RichText::new(plugin.name).size(18.0).strong());
                        ui.label(RichText::new(plugin.about).weak());
                        let newest_text = match (latest, newest(i)) {
                            (None, _) => "Newest: checking…".to_string(),
                            (_, Some(l)) => format!("Newest: {}{}", l.version, if l.prerelease { " (pre-release)" } else { "" }),
                            (Some(Ok(_)), None) => "Newest: not on GitHub yet".to_string(),
                            (Some(Err(_)), None) => "Newest: unknown".to_string(),
                        };
                        ui.label(format!("{}   ·   {newest_text}", version_text(installed)));
                    });
                    ui.with_layout(Layout::top_down(Align::Max), |ui| {
                        let install_label = match (installed, newest(i)) {
                            (Installed::No, Some(_)) => Some("Install".to_string()),
                            (_, Some(l)) if outdated(i, installed) => Some(format!("Update to {}", l.version)),
                            _ => None,
                        };
                        if let Some(label) = install_label {
                            if sized_button(ui, !blocked, Vec2::new(170.0, 30.0), egui::Button::new(RichText::new(label).strong())) {
                                actions.push(Action::Install(vec![i]));
                            }
                        } else if matches!((installed, newest(i)), (Installed::Version(_), Some(_))) {
                            ui.colored_label(GREEN, "✔ Up to date");
                        }
                        if *installed != Installed::No && self.confirm_remove != Some(i) && ui.add_enabled(!blocked, egui::Button::new("Remove").small()).clicked() {
                            actions.push(Action::AskRemove(Some(i)));
                        }
                        if let Some(l) = newest(i) {
                            if ui.link("Release notes").clicked() {
                                actions.push(Action::Open(l.page.clone()));
                            }
                        }
                    });
                });
                if self.confirm_remove == Some(i) {
                    ui.separator();
                    ui.label(format!("Remove {}? Its settings stay in {}.", plugin.name, plugins::settings_dir(plugin.id).display()));
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!blocked, egui::Button::new(RichText::new(format!("Remove {}", plugin.name)).color(RED))).clicked() {
                            actions.push(Action::Remove(i));
                        }
                        if ui.button("Cancel").clicked() {
                            actions.push(Action::AskRemove(None));
                        }
                    });
                }
            });
            ui.add_space(4.0);
        }
    }

    fn browser_section(&mut self, ui: &mut egui::Ui, blocked: bool, actions: &mut Vec<Action>) {
        let state_text = match self.browser {
            Some(BrowserState::Active) => "active",
            Some(BrowserState::Disabled) => "disabled",
            Some(BrowserState::CameBack) => "back after an OBS update",
            Some(BrowserState::Missing) => "not in this OBS",
            None => "OBS not found",
        };
        let control_panel_installed = PLUGINS.iter().position(|p| p.id == "theater-control-panel").is_some_and(|i| self.installed[i] != Installed::No);
        egui::CollapsingHeader::new(RichText::new(format!("Browser Source and Custom Browser Docks (obs-browser): {state_text}")).size(16.0).strong())
            .default_open(self.browser == Some(BrowserState::CameBack))
            .show(ui, |ui| match self.browser {
                Some(BrowserState::Active) | Some(BrowserState::CameBack) => {
                    if self.browser == Some(BrowserState::CameBack) {
                        ui.colored_label(AMBER, "obs-browser is back, probably from an OBS update or reinstall. Disable it again if you still want it off.");
                    }
                    ui.label(
                        "obs-browser is the part of OBS that shows web pages, with its own copy of Chromium. \
                         Theater doesn't need it: the Control Panel's dock and the Theater Web Source use Windows' WebView2.",
                    );
                    ui.add_space(4.0);
                    ui.label(RichText::new("Move your Browser Sources over first, with the Theater Control Panel").strong());
                    ui.label(
                        "🎭 Theater > Convert Browser Sources… turns your Browser Sources into Theater Web Sources. \
                         Only their type changes: name, URL or local file, size, CSS, filters and place in your scenes stay, \
                         and each scene collection is backed up first.",
                    );
                    if !control_panel_installed {
                        ui.colored_label(AMBER, "⚠ The Theater Control Panel isn't installed: install it above to get that tool.");
                    }
                    ui.add_space(6.0);
                    egui::Frame::new()
                        .fill(Color32::from_rgb(58, 18, 18))
                        .stroke(egui::Stroke::new(2.0, RED))
                        .corner_radius(6.0)
                        .inner_margin(14.0)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new("⚠ READ THIS BEFORE DISABLING obs-browser").size(20.0).strong().color(RED));
                            ui.add_space(4.0);
                            ui.label(RichText::new("With obs-browser disabled, OBS shows no web pages itself:").strong());
                            for line in [
                                // (">" rather than an arrow: egui's built-in font has none)
                                "Every Browser Source you didn't convert stops showing. They stay in your scenes as missing sources, with their settings, until obs-browser is back.",
                                "Docks > Custom Browser Docks disappears, with the docks you made there.",
                                "Account connections (Settings > Stream > Connect Account) and their docks stop working: Twitch, YouTube and Restream chat, stats and activity feeds.",
                                "Alerts, chat boxes and widgets from other services or plugins that need a Browser Source stop showing, until you move them to Theater Web Sources.",
                                "Updating or reinstalling OBS may bring obs-browser back. This installer will tell you, and can disable it again.",
                            ] {
                                ui.horizontal_top(|ui| {
                                    ui.label(RichText::new("•").strong().color(RED));
                                    ui.add(egui::Label::new(line).wrap());
                                });
                            }
                            ui.add_space(4.0);
                            ui.label("Nothing is deleted: its files are renamed to .old, and Restore puts them back.");
                            ui.add_space(6.0);
                            ui.checkbox(
                                &mut self.understood,
                                RichText::new(
                                    "I understand: the Browser Sources I didn't convert, my Custom Browser Docks and account connections stop working until I restore obs-browser.",
                                )
                                .strong(),
                            );
                        });
                    ui.add_space(6.0);
                    let label = if self.browser == Some(BrowserState::CameBack) { "Disable obs-browser again" } else { "Disable obs-browser" };
                    let button = egui::Button::new(RichText::new(label).strong().color(Color32::WHITE)).fill(Color32::from_rgb(150, 36, 32));
                    if sized_button(ui, self.understood && !blocked, Vec2::new(240.0, 32.0), button) {
                        actions.push(Action::Browser { disable: true });
                    }
                }
                Some(BrowserState::Disabled) => {
                    ui.label("obs-browser is disabled: its files are renamed to .old. Browser Sources, Custom Browser Docks and account connections are off.");
                    if sized_button(ui, !blocked, Vec2::new(240.0, 32.0), egui::Button::new(RichText::new("Restore obs-browser").strong())) {
                        actions.push(Action::Browser { disable: false });
                    }
                }
                Some(BrowserState::Missing) => {
                    ui.label("This OBS has no obs-browser, so there's nothing to disable.");
                }
                None => {
                    ui.label("OBS Studio wasn't found on this PC. If it's there, choose its folder at the top.");
                }
            });
    }
}

/// A button of a set size, its label centered (min_size would push the label
/// to the side its column is aligned to). True when clicked.
fn sized_button(ui: &mut egui::Ui, enabled: bool, size: Vec2, button: egui::Button) -> bool {
    ui.add_enabled_ui(enabled, |ui| ui.add_sized(size, button)).inner.clicked()
}

/// A line in a colored frame
fn notice(ui: &mut egui::Ui, color: Color32, text: &str) {
    egui::Frame::new().stroke(egui::Stroke::new(1.5, color)).corner_radius(4.0).inner_margin(8.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.colored_label(color, text);
    });
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let changed = std::mem::take(&mut self.shared.lock().unwrap().changed);
        if changed || self.looked_at.elapsed() >= Duration::from_secs(2) {
            self.look();
        }
        ctx.request_repaint_after(Duration::from_secs(2));
        let (latest, busy, log) = {
            let shared = self.shared.lock().unwrap();
            (shared.latest.clone(), shared.busy.clone(), shared.log.clone())
        };
        let blocked = busy.is_some() || self.obs_running;
        let mut actions = Vec::new();

        egui::TopBottomPanel::top("header").show(ctx, |ui| self.header(ui, &latest, &mut actions));

        egui::TopBottomPanel::bottom("log").show(ctx, |ui| {
            ui.add_space(6.0);
            if let Some((what, done)) = &busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(what);
                });
                if let Some(done) = done {
                    ui.add(egui::ProgressBar::new(*done).show_percentage());
                }
            }
            if log.is_empty() && busy.is_none() {
                ui.label(RichText::new("Close OBS before installing, updating or removing. Your settings are never touched.").weak());
            }
            egui::ScrollArea::vertical().max_height(110.0).stick_to_bottom(true).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (error, message) in &log {
                    if *error {
                        ui.colored_label(RED, message);
                    } else {
                        ui.label(message);
                    }
                }
            });
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.plugin_rows(ui, &latest, blocked, &mut actions);
                ui.add_space(12.0);
                self.browser_section(ui, blocked, &mut actions);
                ui.add_space(8.0);
            });
        });

        for action in actions {
            match action {
                Action::Check => self.check_github(ctx),
                Action::Install(which) => {
                    if let Some(Ok(found)) = &latest {
                        self.install(ctx, which, found);
                    }
                }
                Action::AskRemove(which) => self.confirm_remove = which,
                Action::Remove(i) => {
                    self.confirm_remove = None;
                    self.remove(ctx, i);
                }
                Action::Browser { disable } => {
                    self.understood = false;
                    self.set_browser(ctx, disable);
                }
                Action::Open(url) => obs::open_url(&url),
                Action::ChooseObs => {
                    let Some(picked) = obs::pick_folder(self.obs_dir.as_deref()) else { continue };
                    match obs::obs_root(&picked) {
                        Some(dir) => match obs::set_chosen_dir(Some(&dir)) {
                            Ok(()) => {
                                self.note(false, format!("OBS Studio is in {}. The installer will remember it.", dir.display()));
                                self.use_obs(Some(dir), true);
                            }
                            Err(e) => self.note(true, e),
                        },
                        None => self.note(
                            true,
                            format!(
                                "There's no OBS Studio in {}. Pick the folder with bin\\64bit\\obs64.exe in it, like C:\\Program Files\\obs-studio.",
                                picked.display()
                            ),
                        ),
                    }
                }
                Action::FindObs => match obs::set_chosen_dir(None) {
                    Ok(()) => self.use_obs(obs::found_dir(), false),
                    Err(e) => self.note(true, e),
                },
            }
        }
    }
}
