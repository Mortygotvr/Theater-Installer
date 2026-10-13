//! The Theater plugins, where OBS finds them
//! (C:\ProgramData\obs-studio\plugins\<id>\bin\64bit\<id>.dll), and
//! installing, updating and removing them. Only the plugin's own folder
//! changes: settings live in OBS's plugin_config\<id> and stay.

use semver::Version;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub struct Plugin {
    /// Its folder, DLL and release tag name
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub icon: &'static [u8],
}

pub const PLUGINS: [Plugin; 6] = [
    Plugin {
        id: "theater-scene",
        name: "Theater Scene",
        about: "Turns stream events into effects on your avatar: redeems, poses, the Scene Studio, and Rhai chat and alerts. \
                Its alerts come from Theater Reader or a Theater API plugin.",
        icon: include_bytes!("../assets/scene.png"),
    },
    Plugin {
        id: "theater-reader",
        name: "Theater Reader",
        about: "Reads Twitch, Kick and YouTube chat and alerts inside OBS, without logging in, for Theater Scene and the Control Panel.",
        icon: include_bytes!("../assets/reader.png"),
    },
    Plugin {
        id: "theater-control-panel",
        name: "Theater Control Panel",
        about: "Triggers, commands and modules in an OBS dock, and the Theater Web Source, which replaces Browser Source. \
                Its Convert Browser Sources… tool switches your existing Browser Sources over, keeping their settings.",
        icon: include_bytes!("../assets/control-panel.png"),
    },
    Plugin {
        id: "theater-api-twitch",
        name: "Theater API · Twitch",
        about: "Logs in to your Twitch channel: redeems with their real titles, cheers, subs, raids, follows and hype trains, \
                plus your rewards, subscribers and followers. Works with or without Theater Reader.",
        icon: include_bytes!("../assets/api-twitch.png"),
    },
    Plugin {
        id: "theater-api-patreon",
        name: "Theater API · Patreon",
        about: "Logs in to your Patreon campaign: new, changed and ended pledges, and your patrons by tier.",
        icon: include_bytes!("../assets/api-patreon.png"),
    },
    Plugin {
        id: "theater-api-kofi",
        name: "Theater API · Ko-fi",
        about: "Ko-fi donations, memberships, shop orders and commissions, through your Ko-fi webhook.",
        icon: include_bytes!("../assets/api-kofi.png"),
    },
];

/// Written into the plugin's folder: the version installed
const VERSION_FILE: &str = "version.txt";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installed {
    No,
    /// There, but from before the installer (no version.txt)
    Unknown,
    Version(Version),
}

/// Where OBS looks for plugins installed apart from OBS
pub fn plugins_dir() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("obs-studio")
        .join("plugins")
}

/// Where a plugin keeps your settings (the installer never touches it)
pub fn settings_dir(id: &str) -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("obs-studio")
        .join("plugin_config")
        .join(id)
}

fn dll(folder: &Path, id: &str) -> PathBuf {
    folder.join("bin").join("64bit").join(format!("{id}.dll"))
}

pub fn installed(dir: &Path, id: &str) -> Installed {
    let folder = dir.join(id);
    if !dll(&folder, id).is_file() {
        return Installed::No;
    }
    match fs::read_to_string(folder.join(VERSION_FILE)).ok().and_then(|s| Version::parse(s.trim()).ok()) {
        Some(v) => Installed::Version(v),
        None => Installed::Unknown,
    }
}

/// What Windows said, in words that say what to do about it
fn file_error(action: &str, path: &Path, e: io::Error) -> String {
    match e.raw_os_error() {
        Some(5) => format!(
            "Windows refused to {action} {} (access denied). Close any window or program using it. \
             If an installer that ran as administrator put it there, run this installer as administrator \
             (right-click it, Run as administrator).",
            path.display()
        ),
        Some(32) | Some(33) => format!("Couldn't {action} {}: a program is using it. Close OBS and try again.", path.display()),
        _ => format!("Couldn't {action} {}: {e}", path.display()),
    }
}

fn remove_if_there(path: &Path) -> Result<(), String> {
    if path.exists() {
        fs::remove_dir_all(path).map_err(|e| file_error("remove", path, e))?;
    }
    Ok(())
}

/// Installs (or updates) a plugin from its release zip. The zip is unpacked
/// beside the plugins first and the folders are swapped after, so if a step
/// fails the plugin stays as it was, and OBS never finds half of one.
pub fn install_zip(dir: &Path, id: &str, zip_path: &Path, version: &Version) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| file_error("create", dir, e))?;
    let staging = dir.join(format!(".{id}.new"));
    let old = dir.join(format!(".{id}.old"));
    let target = dir.join(id);
    remove_if_there(&staging)?;
    remove_if_there(&old)?;
    let result = (|| {
        unpack(zip_path, id, &staging)?;
        let unpacked = staging.join(id);
        if !dll(&unpacked, id).is_file() {
            return Err(format!("This zip has no {id}.dll in {id}\\bin\\64bit: it isn't a {id} release."));
        }
        fs::write(unpacked.join(VERSION_FILE), format!("{version}\n")).map_err(|e| file_error("write in", &unpacked, e))?;
        if target.exists() {
            fs::rename(&target, &old).map_err(|e| file_error("replace", &target, e))?;
        }
        if let Err(e) = fs::rename(&unpacked, &target) {
            let _ = fs::rename(&old, &target);
            return Err(file_error("create", &target, e));
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_dir_all(&old);
    result
}

/// Unpacks a release zip into `into`. Every file must be inside `<id>\`.
fn unpack(zip_path: &Path, id: &str, into: &Path) -> Result<(), String> {
    let file = fs::File::open(zip_path).map_err(|e| file_error("open", zip_path, e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("The zip couldn't be read: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("The zip couldn't be read: {e}"))?;
        // Windows PowerShell's Compress-Archive puts \ between folders
        let name = entry.name().replace('\\', "/");
        let rel = safe_path(&name).ok_or_else(|| format!("The zip has an unexpected path in it: {name}"))?;
        if rel.components().next() != Some(Component::Normal(id.as_ref())) {
            return Err(format!("The zip has a file outside {id}\\: {name}"));
        }
        let out = into.join(&rel);
        if entry.is_dir() || name.ends_with('/') {
            fs::create_dir_all(&out).map_err(|e| file_error("create", &out, e))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| file_error("create", parent, e))?;
        }
        let mut f = fs::File::create(&out).map_err(|e| file_error("create", &out, e))?;
        io::copy(&mut entry, &mut f).map_err(|e| format!("Couldn't unpack {name}: {e}"))?;
    }
    Ok(())
}

/// A zip entry's path as a relative path, or None when it could land
/// outside the folder it's unpacked in (.., C:, \\server)
fn safe_path(name: &str) -> Option<PathBuf> {
    let mut path = PathBuf::new();
    for part in name.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            p if p.contains(':') => return None,
            p => path.push(p),
        }
    }
    (!path.as_os_str().is_empty()).then_some(path)
}

/// Removes a plugin's folder (all at once: it's renamed first, so OBS never
/// finds half of it). Its settings stay.
pub fn remove(dir: &Path, id: &str) -> Result<(), String> {
    let target = dir.join(id);
    if !target.exists() {
        return Ok(());
    }
    let old = dir.join(format!(".{id}.old"));
    remove_if_there(&old)?;
    fs::rename(&target, &old).map_err(|e| file_error("remove", &target, e))?;
    fs::remove_dir_all(&old).map_err(|e| file_error("remove", &old, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("theater-installer-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A zip like the release zips, with \ between folders like Compress-Archive makes them
    fn make_zip(path: &Path, files: &[(&str, &str)]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, content) in files {
            zip.start_file(name.to_string(), zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn installs_then_updates_replacing_the_whole_folder() {
        let tmp = temp_dir("update");
        let plugins = tmp.join("plugins");
        let v1 = tmp.join("v1.zip");
        make_zip(&v1, &[(r"theater-scene\bin\64bit\theater-scene.dll", "one"), (r"theater-scene\data\old-only.txt", "gone after the update")]);
        install_zip(&plugins, "theater-scene", &v1, &Version::new(0, 1, 0)).unwrap();
        assert_eq!(installed(&plugins, "theater-scene"), Installed::Version(Version::new(0, 1, 0)));

        let v2 = tmp.join("v2.zip");
        make_zip(&v2, &[("theater-scene/bin/64bit/theater-scene.dll", "two"), ("theater-scene/data/new.txt", "new")]);
        install_zip(&plugins, "theater-scene", &v2, &Version::new(0, 2, 0)).unwrap();
        assert_eq!(installed(&plugins, "theater-scene"), Installed::Version(Version::new(0, 2, 0)));
        assert_eq!(fs::read_to_string(plugins.join(r"theater-scene\bin\64bit\theater-scene.dll")).unwrap(), "two");
        assert!(!plugins.join(r"theater-scene\data\old-only.txt").exists(), "files the new version doesn't have are gone");
        assert!(!plugins.join(".theater-scene.new").exists() && !plugins.join(".theater-scene.old").exists());

        remove(&plugins, "theater-scene").unwrap();
        assert_eq!(installed(&plugins, "theater-scene"), Installed::No);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_bad_zip_leaves_the_installed_plugin_as_it_was() {
        let tmp = temp_dir("bad");
        let plugins = tmp.join("plugins");
        let good = tmp.join("good.zip");
        make_zip(&good, &[("theater-reader/bin/64bit/theater-reader.dll", "good")]);
        install_zip(&plugins, "theater-reader", &good, &Version::new(0, 1, 0)).unwrap();

        let wrong = tmp.join("wrong.zip");
        make_zip(&wrong, &[("theater-scene/bin/64bit/theater-scene.dll", "not the reader")]);
        assert!(install_zip(&plugins, "theater-reader", &wrong, &Version::new(0, 2, 0)).is_err());
        let escape = tmp.join("escape.zip");
        make_zip(&escape, &[("theater-reader/bin/64bit/theater-reader.dll", "x"), (r"theater-reader\..\..\evil.txt", "x")]);
        assert!(install_zip(&plugins, "theater-reader", &escape, &Version::new(0, 2, 0)).is_err());
        assert!(!tmp.join("evil.txt").exists() && !plugins.join("evil.txt").exists());
        let no_dll = tmp.join("nodll.zip");
        make_zip(&no_dll, &[("theater-reader/data/icon.png", "x")]);
        assert!(install_zip(&plugins, "theater-reader", &no_dll, &Version::new(0, 2, 0)).is_err());

        assert_eq!(installed(&plugins, "theater-reader"), Installed::Version(Version::new(0, 1, 0)));
        assert_eq!(fs::read_to_string(plugins.join(r"theater-reader\bin\64bit\theater-reader.dll")).unwrap(), "good");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_plugin_from_before_the_installer_has_an_unknown_version() {
        let tmp = temp_dir("unknown");
        fs::create_dir_all(tmp.join(r"theater-control-panel\bin\64bit")).unwrap();
        fs::write(tmp.join(r"theater-control-panel\bin\64bit\theater-control-panel.dll"), "x").unwrap();
        assert_eq!(installed(&tmp, "theater-control-panel"), Installed::Unknown);
        assert_eq!(installed(&tmp, "theater-scene"), Installed::No);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn zip_paths_stay_inside() {
        assert_eq!(safe_path("a/b/c.txt"), Some(PathBuf::from(r"a\b\c.txt")));
        assert_eq!(safe_path("a/./b/"), Some(PathBuf::from(r"a\b")));
        assert_eq!(safe_path("a/../b"), None);
        assert_eq!(safe_path("C:/Windows/x"), None);
        assert_eq!(safe_path("/"), None);
    }
}
