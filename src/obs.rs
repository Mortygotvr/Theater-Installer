//! OBS Studio on this PC: where it's installed, its version, whether it's
//! running, and its obs-browser plugin (Browser Source and Custom Browser
//! Docks). obs-browser is disabled by renaming its files to .old, as you'd do
//! by hand, and restored by renaming them back: nothing is deleted.

use std::ffi::c_void;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, ERROR_SUCCESS};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, ShellExecuteExW, ShellExecuteW, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SEE_MASK_NOASYNC,
    SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL};

/// The installer's command line when it runs again as administrator for an
/// obs-browser step: --obs-browser <disable|restore> <OBS folder> <error file>
pub const ELEVATED_ARG: &str = "--obs-browser";
const ERROR_FILE: &str = "theater-installer-obs-browser.txt";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn reg_string(root: HKEY, key: &str, value: Option<&str>) -> Option<String> {
    let key = wide(key);
    let value = value.map(wide);
    let value_ptr = value.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
    let mut size = 0u32;
    unsafe {
        if RegGetValueW(root, PCWSTR(key.as_ptr()), value_ptr, RRF_RT_REG_SZ, None, None, Some(&mut size)) != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; size as usize / 2 + 1];
        if RegGetValueW(root, PCWSTR(key.as_ptr()), value_ptr, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut c_void), Some(&mut size)) != ERROR_SUCCESS {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

/// Where OBS is installed: the folder you chose, else what OBS's installer
/// wrote in the registry, the usual folder, or Steam's copy
pub fn install_dir() -> Option<PathBuf> {
    chosen_dir().or_else(found_dir)
}

/// The OBS found without your help
pub fn found_dir() -> Option<PathBuf> {
    let usual = std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("obs-studio"));
    reg_string(HKEY_LOCAL_MACHINE, r"SOFTWARE\OBS Studio", None)
        .map(PathBuf::from)
        .into_iter()
        .chain(usual)
        .chain(steam_dirs())
        .find(|dir| is_obs(dir))
}

fn is_obs(dir: &Path) -> bool {
    dir.join(r"bin\64bit\obs64.exe").is_file()
}

/// OBS from Steam, in each Steam library
fn steam_dirs() -> Vec<PathBuf> {
    let Some(steam) = reg_string(HKEY_CURRENT_USER, r"Software\Valve\Steam", Some("SteamPath")).map(PathBuf::from) else { return Vec::new() };
    let mut libraries = vec![steam.clone()];
    // "path"		"D:\SteamLibrary" lines in libraryfolders.vdf
    if let Ok(vdf) = fs::read_to_string(steam.join(r"steamapps\libraryfolders.vdf")) {
        for line in vdf.lines() {
            let parts: Vec<&str> = line.split('"').collect();
            if let [_, "path", _, path, ..] = parts[..] {
                libraries.push(PathBuf::from(path.replace(r"\\", r"\")));
            }
        }
    }
    libraries.into_iter().map(|lib| lib.join(r"steamapps\common\OBS Studio")).collect()
}

/// The installer's own settings: the OBS folder you chose
fn chosen_file() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Theater-Installer").join("obs-folder.txt"))
}

/// The OBS folder you chose, while it's still an OBS folder
pub fn chosen_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(fs::read_to_string(chosen_file()?).ok()?.trim());
    is_obs(&dir).then_some(dir)
}

/// Remembers the OBS folder you chose, or forgets it (None)
pub fn set_chosen_dir(dir: Option<&Path>) -> Result<(), String> {
    let file = chosen_file().ok_or("Windows didn't say where to keep the installer's settings (LOCALAPPDATA).")?;
    let result = match dir {
        Some(dir) => fs::create_dir_all(file.parent().unwrap()).and_then(|_| fs::write(&file, dir.display().to_string())),
        None => match fs::remove_file(&file) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        },
    };
    result.map_err(|e| format!("Couldn't save the OBS folder in {}: {e}", file.display()))
}

/// The OBS folder in a folder you picked: that folder, or the one above its
/// bin\64bit (where obs64.exe is)
pub fn obs_root(picked: &Path) -> Option<PathBuf> {
    picked.ancestors().take(3).find(|dir| is_obs(dir)).map(Path::to_path_buf)
}

/// Windows' folder picker. None when cancelled.
pub fn pick_folder(start: Option<&Path>) -> Option<PathBuf> {
    unsafe {
        // Already done by the window on this thread, most likely: either way is fine
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let options = dialog.GetOptions().ok()?;
        dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        let title = wide("Choose the OBS Studio folder (the one with bin, data and obs-plugins in it)");
        let _ = dialog.SetTitle(PCWSTR(title.as_ptr()));
        if let Some(start) = start {
            let start = wide(&start.display().to_string());
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(start.as_ptr()), None) {
                let _ = dialog.SetFolder(&item);
            }
        }
        dialog.Show(None).ok()?;
        let name = dialog.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const c_void));
        path.map(PathBuf::from)
    }
}

/// OBS's version ("32.0.1"): from its obs64.exe, else as its installer
/// recorded it
pub fn version(dir: &Path) -> Option<String> {
    exe_version(&dir.join(r"bin\64bit\obs64.exe")).or_else(|| {
        [r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\OBS Studio", r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\OBS Studio"]
            .iter()
            .find_map(|key| reg_string(HKEY_LOCAL_MACHINE, key, Some("DisplayVersion")))
            .filter(|v| !v.trim().is_empty())
    })
}

fn exe_version(exe: &Path) -> Option<String> {
    let name = wide(&exe.display().to_string());
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(name.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(name.as_ptr()), None, size, data.as_mut_ptr() as *mut c_void).ok()?;
        let (mut info, mut len) = (std::ptr::null_mut::<c_void>(), 0u32);
        let root = wide("\\");
        if !VerQueryValueW(data.as_ptr() as *const c_void, PCWSTR(root.as_ptr()), &mut info, &mut len).as_bool() || info.is_null() {
            return None;
        }
        let info = &*(info as *const VS_FIXEDFILEINFO);
        let (ms, ls) = (info.dwProductVersionMS, info.dwProductVersionLS);
        (ms != 0).then(|| format!("{}.{}.{}", ms >> 16, ms & 0xffff, ls >> 16))
    }
}

pub fn is_running() -> bool {
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = false;
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case("obs64.exe") {
                    found = true;
                    break;
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserState {
    Active,
    Disabled,
    /// Active again while the renamed copy is still there: an OBS update or
    /// reinstall put it back
    CameBack,
    /// This OBS has no obs-browser
    Missing,
}

/// The dll first: once it's renamed, OBS no longer loads the plugin
const BROWSER_FILES: [&str; 4] = ["obs-browser.dll", "obs-browser-page.exe", "obs-browser.pdb", "obs-browser-page.pdb"];

fn browser_dir(obs: &Path) -> PathBuf {
    obs.join("obs-plugins").join("64bit")
}

fn renamed(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

pub fn browser_state(obs: &Path) -> BrowserState {
    let dll = browser_dir(obs).join(BROWSER_FILES[0]);
    match (dll.is_file(), renamed(&dll).is_file()) {
        (true, true) => BrowserState::CameBack,
        (true, false) => BrowserState::Active,
        (false, true) => BrowserState::Disabled,
        (false, false) => BrowserState::Missing,
    }
}

/// Renames obs-browser's files to .old (disable) or back (restore). Disabling
/// renames the dll first and restoring puts it back last, so if a step fails,
/// OBS still finds either all of the plugin or none of it.
pub fn set_browser(obs: &Path, disable: bool) -> io::Result<()> {
    let dir = browser_dir(obs);
    if disable {
        for name in BROWSER_FILES {
            let file = dir.join(name);
            if file.is_file() {
                let old = renamed(&file);
                if old.is_file() {
                    fs::remove_file(&old)?; // the copy from before an OBS update
                }
                fs::rename(&file, &old)?;
            }
        }
    } else {
        for name in BROWSER_FILES.iter().rev() {
            let file = dir.join(name);
            let old = renamed(&file);
            if old.is_file() && !file.exists() {
                fs::rename(&old, &file)?;
            }
        }
    }
    Ok(())
}

/// Disables or restores obs-browser. OBS's folder is in Program Files, so
/// when Windows refuses, the installer runs again as administrator for this
/// step only (Windows asks you first) and waits for it.
pub fn set_browser_as_needed(obs: &Path, disable: bool) -> Result<(), String> {
    match set_browser(obs, disable) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => set_browser_elevated(obs, disable),
        Err(e) => Err(format!("Couldn't rename obs-browser's files: {e}")),
    }
}

fn set_browser_elevated(obs: &Path, disable: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Couldn't find the installer's own file: {e}"))?;
    let error_file = std::env::temp_dir().join(ERROR_FILE);
    let _ = fs::remove_file(&error_file);
    // No \ before a closing quote: Windows would read it as an escaped quote
    let quoted = |p: &Path| format!("\"{}\"", p.display().to_string().trim_end_matches('\\'));
    let params = format!("{ELEVATED_ARG} {} {} {}", if disable { "disable" } else { "restore" }, quoted(obs), quoted(&error_file));
    let (verb, file, params) = (wide("runas"), wide(&exe.display().to_string()), wide(&params));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
            "Windows' permission wasn't given, so obs-browser wasn't changed.".to_string()
        } else {
            format!("Couldn't ask Windows for permission: {e}")
        });
    }
    let mut code = 1u32;
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
    }
    let detail = fs::read_to_string(&error_file).ok();
    let _ = fs::remove_file(&error_file);
    match code {
        0 => Ok(()),
        _ => Err(detail.unwrap_or_else(|| format!("The step as administrator failed (code {code})."))),
    }
}

/// The step as administrator (see ELEVATED_ARG); returns the exit code
pub fn run_elevated_step(args: &[String]) -> i32 {
    let [action, obs, error_file] = args else { return 2 };
    let obs = Path::new(obs);
    let error_file = Path::new(error_file);
    // Only obs-browser's own files in an OBS folder, and only our error file
    if !obs.join(r"bin\64bit\obs64.exe").is_file() || error_file.file_name().is_none_or(|n| n != ERROR_FILE) {
        return 2;
    }
    let result = match action.as_str() {
        "disable" => set_browser(obs, true),
        "restore" => set_browser(obs, false),
        _ => return 2,
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            let _ = fs::write(error_file, format!("Couldn't rename obs-browser's files: {e}"));
            1
        }
    }
}

/// Opens a web page in your browser, or a folder in Explorer
pub fn open_url(url: &str) {
    let (verb, url) = (wide("open"), wide(url));
    unsafe {
        ShellExecuteW(None, PCWSTR(verb.as_ptr()), PCWSTR(url.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_obs(name: &str, files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("theater-installer-obs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(browser_dir(&dir)).unwrap();
        for f in files {
            fs::write(browser_dir(&dir).join(f), f).unwrap();
        }
        dir
    }

    #[test]
    fn disables_and_restores_by_renaming() {
        let obs = fake_obs("toggle", &["obs-browser.dll", "obs-browser-page.exe", "obs-browser.pdb", "libcef.dll"]);
        assert_eq!(browser_state(&obs), BrowserState::Active);
        set_browser(&obs, true).unwrap();
        assert_eq!(browser_state(&obs), BrowserState::Disabled);
        let d = browser_dir(&obs);
        assert!(d.join("obs-browser-page.exe.old").is_file() && d.join("obs-browser.pdb.old").is_file());
        assert!(d.join("libcef.dll").is_file(), "Chromium's files stay");
        set_browser(&obs, false).unwrap();
        assert_eq!(browser_state(&obs), BrowserState::Active);
        assert!(d.join("obs-browser-page.exe").is_file() && !d.join("obs-browser-page.exe.old").exists());
        let _ = fs::remove_dir_all(&obs);
    }

    #[test]
    fn an_obs_update_that_brings_it_back_is_noticed_and_disabled_again() {
        let obs = fake_obs("cameback", &["obs-browser.dll", "obs-browser.dll.old", "obs-browser-page.exe", "obs-browser-page.exe.old"]);
        assert_eq!(browser_state(&obs), BrowserState::CameBack);
        set_browser(&obs, true).unwrap();
        assert_eq!(browser_state(&obs), BrowserState::Disabled);
        assert_eq!(fs::read_to_string(browser_dir(&obs).join("obs-browser.dll.old")).unwrap(), "obs-browser.dll", "the newer copy is kept");
        let _ = fs::remove_dir_all(&obs);
    }

    #[test]
    fn restore_never_overwrites() {
        let obs = fake_obs("keep", &["obs-browser.dll", "obs-browser.dll.old"]);
        set_browser(&obs, false).unwrap();
        assert_eq!(fs::read_to_string(browser_dir(&obs).join("obs-browser.dll")).unwrap(), "obs-browser.dll");
        let none = fake_obs("none", &["libcef.dll"]);
        assert_eq!(browser_state(&none), BrowserState::Missing);
        let _ = fs::remove_dir_all(&obs);
        let _ = fs::remove_dir_all(&none);
    }

    #[test]
    fn a_picked_folder_inside_obs_finds_obs() {
        let obs = fake_obs("picked", &[]);
        fs::create_dir_all(obs.join(r"bin\64bit")).unwrap();
        fs::write(obs.join(r"bin\64bit\obs64.exe"), "").unwrap();
        for picked in [obs.clone(), obs.join("bin"), obs.join(r"bin\64bit"), obs.join("obs-plugins")] {
            assert_eq!(obs_root(&picked).as_deref(), Some(obs.as_path()), "{}", picked.display());
        }
        assert_eq!(obs_root(&std::env::temp_dir()), None, "no OBS there");
        let _ = fs::remove_dir_all(&obs);
    }

    #[test]
    fn reads_an_exes_version() {
        let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap();
        let version = exe_version(&windir.join("explorer.exe")).unwrap();
        assert!(version.starts_with("10.") || version.starts_with("11."), "{version}");
        assert_eq!(exe_version(&windir.join("no-such-file.exe")), None);
    }

    #[test]
    fn the_elevated_step_refuses_anything_but_obs_and_its_error_file() {
        let not_obs = fake_obs("notobs", &["obs-browser.dll"]);
        let error_file = std::env::temp_dir().join(ERROR_FILE).display().to_string();
        let args = |a: &str, dir: &Path, e: &str| vec![a.to_string(), dir.display().to_string(), e.to_string()];
        assert_eq!(run_elevated_step(&args("disable", &not_obs, &error_file)), 2, "no obs64.exe there");
        fs::create_dir_all(not_obs.join(r"bin\64bit")).unwrap();
        fs::write(not_obs.join(r"bin\64bit\obs64.exe"), "").unwrap();
        assert_eq!(run_elevated_step(&args("disable", &not_obs, r"C:\Windows\win.ini")), 2);
        assert_eq!(run_elevated_step(&args("delete", &not_obs, &error_file)), 2);
        assert_eq!(run_elevated_step(&args("disable", &not_obs, &error_file)), 0);
        assert_eq!(browser_state(&not_obs), BrowserState::Disabled);
        let _ = fs::remove_dir_all(&not_obs);
    }
}
