//! Theater's releases on GitHub. One public repo holds every plugin's zip, one
//! release per plugin version, tagged "<plugin id>-v<version>"
//! (theater-scene-v0.1.0, theater-installer-v0.1.0…). Pre-releases count:
//! while Theater is 0.x, every release is one.

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

pub const RELEASES_REPO: &str = "Mortygotvr/Theater-Releases";

#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
    /// "sha256:<hex>", when GitHub lists it
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

/// A plugin's (or the installer's) newest release
#[derive(Clone, Debug)]
pub struct Latest {
    pub version: Version,
    pub prerelease: bool,
    /// The release's page, with its notes
    pub page: String,
    pub asset: Asset,
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("Theater-Installer/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| format!("Couldn't start the connection: {e}"))
}

/// The newest release of each of `wanted` ((id, asset name ending)), by
/// version, not date. Missing from the map: no release of it yet.
pub fn latest(wanted: &[(&str, &str)]) -> Result<HashMap<String, Latest>, String> {
    let url = format!("https://api.github.com/repos/{RELEASES_REPO}/releases?per_page=100");
    let resp = client()?
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(Duration::from_secs(30))
        .send()
        .map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
    let releases: Vec<Release> = match resp.status().as_u16() {
        200 => resp.json().map_err(|e| format!("GitHub's answer couldn't be read: {e}"))?,
        403 | 429 => return Err("GitHub is limiting requests from this connection. Try again in a while.".into()),
        404 => return Err(format!("The releases ({RELEASES_REPO}) weren't found on GitHub.")),
        code => return Err(format!("GitHub answered with error {code}.")),
    };
    Ok(newest_of(&releases, wanted))
}

fn newest_of(releases: &[Release], wanted: &[(&str, &str)]) -> HashMap<String, Latest> {
    let mut found = HashMap::new();
    for &(id, ending) in wanted {
        let prefix = format!("{id}-v");
        let newest = releases
            .iter()
            .filter(|r| !r.draft)
            .filter_map(|r| {
                let version = Version::parse(r.tag_name.strip_prefix(&prefix)?).ok()?;
                let asset = r.assets.iter().find(|a| a.name.ends_with(ending))?.clone();
                Some(Latest { version, prerelease: r.prerelease, page: r.html_url.clone(), asset })
            })
            .max_by(|a, b| a.version.cmp(&b.version));
        if let Some(latest) = newest {
            found.insert(id.to_string(), latest);
        }
    }
    found
}

/// Downloads an asset to `dest`, telling `progress` how far along it is
/// (0 to 1), and checks it against the checksum GitHub lists for it.
pub fn download(asset: &Asset, dest: &Path, progress: &dyn Fn(f32)) -> Result<(), String> {
    let mut resp = client()?
        .get(&asset.browser_download_url)
        .send()
        .map_err(|e| format!("Couldn't download {}: {e}", asset.name))?;
    if !resp.status().is_success() {
        return Err(format!("Couldn't download {}: error {}.", asset.name, resp.status().as_u16()));
    }
    let total = resp.content_length().unwrap_or(asset.size).max(1);
    let mut file = std::fs::File::create(dest).map_err(|e| format!("Couldn't save {}: {e}", dest.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        let n = resp.read(&mut buf).map_err(|e| format!("The download of {} broke off: {e}", asset.name))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("Couldn't save {}: {e}", dest.display()))?;
        hasher.update(&buf[..n]);
        done += n as u64;
        progress((done as f32 / total as f32).min(1.0));
    }
    file.flush().map_err(|e| format!("Couldn't save {}: {e}", dest.display()))?;
    drop(file);
    if let Some(expected) = asset.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) {
        if !format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(expected) {
            let _ = std::fs::remove_file(dest);
            return Err(format!("{} arrived damaged (its checksum doesn't match). Try again.", asset.name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool, draft: bool, assets: &[&str]) -> Release {
        Release {
            tag_name: tag.into(),
            draft,
            prerelease,
            html_url: format!("https://github.com/{RELEASES_REPO}/releases/tag/{tag}"),
            assets: assets
                .iter()
                .map(|n| Asset { name: n.to_string(), browser_download_url: format!("https://example.invalid/{n}"), size: 1, digest: None })
                .collect(),
        }
    }

    #[test]
    fn picks_each_plugins_newest_version_not_the_newest_release() {
        let releases = vec![
            release("theater-reader-v0.1.1", true, false, &["theater-reader-0.1.1-windows-x64.zip"]),
            release("theater-scene-v0.1.0", true, false, &["theater-scene-0.1.0-windows-x64.zip"]),
            release("theater-scene-v0.2.0", false, false, &["theater-scene-0.2.0-windows-x64.zip"]),
            release("theater-scene-v0.10.0", true, false, &["theater-scene-0.10.0-windows-x64.zip"]),
            release("theater-installer-v0.1.0", true, false, &["Theater-Installer.exe"]),
        ];
        let found = newest_of(
            &releases,
            &[("theater-scene", "-windows-x64.zip"), ("theater-reader", "-windows-x64.zip"), ("theater-control-panel", "-windows-x64.zip"), ("theater-installer", ".exe")],
        );
        assert_eq!(found["theater-scene"].version, Version::new(0, 10, 0), "0.10 is newer than 0.2, whatever the order");
        assert!(found["theater-scene"].prerelease);
        assert_eq!(found["theater-reader"].version, Version::new(0, 1, 1));
        assert_eq!(found["theater-installer"].asset.name, "Theater-Installer.exe");
        assert!(!found.contains_key("theater-control-panel"), "no release of it yet");
    }

    #[test]
    fn skips_drafts_releases_without_the_zip_and_other_tags() {
        let releases = vec![
            release("theater-scene-v0.3.0", false, true, &["theater-scene-0.3.0-windows-x64.zip"]),
            release("theater-scene-v0.2.0", false, false, &["notes.txt"]),
            release("theater-scene-extra-v9.0.0", false, false, &["theater-scene-extra-9.0.0-windows-x64.zip"]),
            release("theater-scene-vbroken", false, false, &["theater-scene-windows-x64.zip"]),
            release("theater-scene-v0.1.0", true, false, &["theater-scene-0.1.0-windows-x64.zip"]),
        ];
        let found = newest_of(&releases, &[("theater-scene", "-windows-x64.zip")]);
        assert_eq!(found["theater-scene"].version, Version::new(0, 1, 0));
    }

    #[test]
    fn reads_githubs_release_list() {
        let json = r#"[{"tag_name":"theater-scene-v0.1.0","draft":false,"prerelease":true,
            "html_url":"https://github.com/Mortygotvr/Theater-Releases/releases/tag/theater-scene-v0.1.0",
            "name":"Theater Scene 0.1.0","body":"notes","assets":[{"name":"theater-scene-0.1.0-windows-x64.zip",
            "browser_download_url":"https://github.com/x.zip","size":123,"digest":"sha256:abcd","content_type":"application/zip"}]}]"#;
        let releases: Vec<Release> = serde_json::from_str(json).unwrap();
        let found = newest_of(&releases, &[("theater-scene", "-windows-x64.zip")]);
        assert_eq!(found["theater-scene"].asset.digest.as_deref(), Some("sha256:abcd"));
        assert_eq!(found["theater-scene"].asset.size, 123);
    }
}
