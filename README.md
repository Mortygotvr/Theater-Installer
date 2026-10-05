# Theater Installer

One program to install, update and remove the Theater plugins for OBS Studio on Windows:

- **Theater Scene**, **Theater Reader** and **Theater Control Panel.** For each one it shows the installed version and the newest one on GitHub, with **Install**, **Update** or **Remove**.
- **obs-browser** (OBS's Browser Source and Custom Browser Docks). It can be disabled, behind a disclaimer, and restored.

## How it works

- **Releases** come from the public [Theater-Releases](https://github.com/Mortygotvr/Theater-Releases) repo. It has one release per plugin version, tagged `<plugin>-v<version>`: `theater-scene-v0.1.0`, `theater-reader-v0.1.0`, `theater-control-panel-v0.1.0`, `theater-installer-v0.1.0`. Each plugin release carries its `<plugin>-<version>-windows-x64.zip`; the installer's release carries `Theater-Installer.exe`. Pre-releases count. The newest version wins, whatever the release dates.
- **Installing** puts the plugin in `C:\ProgramData\obs-studio\plugins\<plugin>`:
  1. The zip is checked against the SHA-256 that GitHub lists for it.
  2. It's unpacked beside the plugins.
  3. The folders are swapped, so a failed step leaves the plugin as it was.
  4. A `version.txt` in the plugin's folder records the version.

  Settings in `%APPDATA%\obs-studio\plugin_config\<plugin>` are never touched, not even by **Remove**.
- **OBS must be closed** to install, update or remove. The installer notices when OBS is open and never closes it.
- **obs-browser:** disabling renames `obs-browser.dll`, `obs-browser-page.exe` and their `.pdb` files in `obs-plugins\64bit` to `.old`. Restoring renames them back. Chromium's files stay where they are.
  - That folder is in Program Files, so the installer runs again as administrator for that step only (`Theater-Installer.exe --obs-browser <disable|restore> <OBS folder> <error file>`). Windows asks you first.
  - If an OBS update brings obs-browser back, the installer shows it and offers to disable it again.

## Build

```powershell
cargo build --release   # target\release\Theater-Installer.exe
cargo test
```

The icon and the manifest (asInvoker, per-monitor DPI) are compiled in from `assets\installer.rc`; that needs the Windows SDK's `rc.exe`.

## Releasing a plugin

1. Build the plugin's zip with its package script.
2. Create a release in Theater-Releases tagged `<plugin>-v<version>` (pre-release while it's 0.x).
3. Attach the zip.

The installer picks the release up the next time it checks.

## License

GPL-2.0-or-later, like the Theater plugins.
