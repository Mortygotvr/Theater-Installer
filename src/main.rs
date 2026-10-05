#![windows_subsystem = "windows"]
//! Theater Installer: installs, updates and removes the Theater plugins for
//! OBS Studio from their GitHub releases, and can disable (and restore) OBS's
//! obs-browser.

mod app;
mod github;
mod obs;
mod plugins;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some(obs::ELEVATED_ARG) {
        std::process::exit(obs::run_elevated_step(&args[1..]));
    }
    app::run()
}
