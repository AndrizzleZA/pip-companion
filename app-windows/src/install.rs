//! Installing Pip on this PC, without an installer.
//!
//! Pip is a single executable, so it installs itself: copy to `%LOCALAPPDATA%\Programs\Pip`,
//! make a Start-menu shortcut, register an entry in Apps & features, and start with Windows from
//! there. Everything is per-user, so no administrator rights and no UAC prompt — and uninstalling
//! puts the machine back exactly as it was.

use crate::platform::wide;
use std::path::{Path, PathBuf};
use windows_sys::Win32::System::Registry::HKEY;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const APP_NAME: &str = "Pip";
const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Pip";

/// `%LOCALAPPDATA%\Programs\Pip` — the per-user equivalent of Program Files.
pub fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Programs").join(APP_NAME)
}

pub fn installed_exe() -> PathBuf {
    install_dir().join("pip.exe")
}

fn current_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_default()
}

/// Is this copy the installed one? (Paths are compared case-insensitively, as Windows does.)
pub fn running_installed() -> bool {
    let (a, b) = (current_exe(), installed_exe());
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

/// The folder this copy runs from, for the About page.
pub fn current_folder() -> String {
    current_exe().parent().map(|p| p.display().to_string()).unwrap_or_default()
}

fn start_menu_shortcut() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Microsoft\\Windows\\Start Menu\\Programs\\Pip.lnk")
}

/// Creates (or replaces) the Start-menu shortcut. Writing a .lnk means COM plumbing, so this
/// hands the job to the shell scripting host once, at install time — nothing at runtime.
fn write_shortcut(target: &Path) -> bool {
    let link = start_menu_shortcut();
    let _ = std::fs::create_dir_all(link.parent().unwrap_or(Path::new(".")));
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut(\"{}\"); $s.TargetPath=\"{}\"; $s.WorkingDirectory=\"{}\"; $s.Description=\"Pip - your tiny desktop companion\"; $s.Save()",
        link.display(),
        target.display(),
        target.parent().map(|p| p.display().to_string()).unwrap_or_default()
    );
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
        .status()
        .map(|st| st.success())
        .unwrap_or(false)
}

fn set_string(key: HKEY, name: &str, value: &str) {
    let (n, v) = (wide(name), wide(value));
    unsafe { RegSetValueExW(key, n.as_ptr(), 0, REG_SZ, v.as_ptr() as *const u8, (v.len() * 2) as u32) };
}

fn set_dword(key: HKEY, name: &str, value: u32) {
    let n = wide(name);
    unsafe { RegSetValueExW(key, n.as_ptr(), 0, REG_DWORD, &value as *const u32 as *const u8, 4) };
}

/// The "Apps & features" / "Add or remove programs" entry, so Pip uninstalls like any other app.
fn write_uninstall_entry(exe: &Path, size_kb: u32) -> bool {
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        let mut disp = 0u32;
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(UNINSTALL_KEY).as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            &mut disp,
        ) != 0
        {
            return false;
        }
        let exe_s = exe.to_string_lossy().to_string();
        set_string(key, "DisplayName", "Pip");
        set_string(key, "DisplayVersion", env!("CARGO_PKG_VERSION"));
        set_string(key, "Publisher", "Andre Kruse");
        set_string(key, "DisplayIcon", &exe_s);
        set_string(key, "InstallLocation", &install_dir().to_string_lossy());
        set_string(key, "UninstallString", &format!("\"{exe_s}\" --uninstall"));
        set_string(key, "QuietUninstallString", &format!("\"{exe_s}\" --uninstall --silent"));
        set_string(key, "URLInfoAbout", "https://sites.google.com/view/pip-companion");
        set_dword(key, "EstimatedSize", size_kb);
        set_dword(key, "NoModify", 1);
        set_dword(key, "NoRepair", 1);
        RegCloseKey(key);
        true
    }
}

fn remove_uninstall_entry() {
    unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, wide(UNINSTALL_KEY).as_ptr()) };
}

pub struct Report {
    pub message: String,
}

/// Copies this executable into place and registers it. Returns where it landed.
pub fn install(start_with_windows: bool) -> Result<PathBuf, String> {
    let src = current_exe();
    let dst = installed_exe();
    if src == dst {
        return Ok(dst);
    }
    std::fs::create_dir_all(install_dir()).map_err(|e| format!("Couldn't create {}: {e}", install_dir().display()))?;
    // A running older copy can't be overwritten; ask it to quit first, then retry briefly.
    if dst.exists() {
        crate::send_command_to(&dst, "quit");
        for _ in 0..20 {
            if std::fs::copy(&src, &dst).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }
    std::fs::copy(&src, &dst).map_err(|e| format!("Couldn't copy Pip to {}: {e}", dst.display()))?;
    let size_kb = std::fs::metadata(&dst).map(|m| (m.len() / 1024) as u32).unwrap_or(2048);
    write_shortcut(&dst);
    write_uninstall_entry(&dst, size_kb);
    if start_with_windows {
        crate::platform::set_autostart_to(&dst, true);
    }
    Ok(dst)
}

/// Removes everything install() created. `keep_data` leaves %APPDATA%\Pip alone.
pub fn uninstall(keep_data: bool) -> Report {
    crate::platform::set_autostart(false);
    let _ = std::fs::remove_file(start_menu_shortcut());
    remove_uninstall_entry();
    if !keep_data {
        if let Some(dir) = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Pip")) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
    // The running copy can't delete itself, so a detached shell does it a moment later.
    // raw_arg: cmd.exe wants the command line verbatim, not Rust's usual quote escaping.
    let exe = current_exe();
    if exe.starts_with(install_dir()) {
        use std::os::windows::process::CommandExt;
        let line = format!(
            "/c ping -n 3 127.0.0.1 >nul & del /f /q \"{}\" & rmdir \"{}\"",
            exe.display(),
            install_dir().display()
        );
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("cmd").raw_arg(&line).creation_flags(CREATE_NO_WINDOW).spawn();
    }
    Report { message: "Pip has been removed.".into() }
}

/// Offers to install when Pip is started from a folder (a download, a USB stick, the build dir).
/// Answering No just runs Pip from where it is, so nothing is forced.
pub fn offer_install_if_loose() -> bool {
    if running_installed() || std::env::args().any(|a| a == "--portable" || a == "--autostart" || a == "--uninstall") {
        return false;
    }
    let text = wide(
        "Install Pip on this PC?\n\nPip will be copied to your account's programs folder, added to the Start menu, and started with Windows.\n\nChoose No to just run it from here.",
    );
    let title = wide("Pip");
    let answer = unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_YESNO | MB_ICONQUESTION) };
    if answer != IDYES {
        return false;
    }
    match install(true) {
        Ok(exe) => {
            let _ = std::process::Command::new(&exe).spawn();
            true
        }
        Err(e) => {
            let msg = wide(&format!("Pip couldn't install itself.\n\n{e}\n\nIt will run from here instead."));
            unsafe { MessageBoxW(std::ptr::null_mut(), msg.as_ptr(), title.as_ptr(), MB_OK | MB_ICONWARNING) };
            false
        }
    }
}

/// `--uninstall` from Apps & features (or the About page).
pub fn run_uninstall(silent: bool) {
    let title = wide("Pip");
    if !silent {
        let text = wide("Remove Pip from this PC?\n\nYour settings and connections will be deleted too.");
        if unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_YESNO | MB_ICONQUESTION) } != IDYES {
            return;
        }
    }
    // Ask a running copy to quit so its files aren't locked.
    crate::send_command_to(&installed_exe(), "quit");
    std::thread::sleep(std::time::Duration::from_millis(400));
    let r = uninstall(false);
    if !silent {
        let msg = wide(&r.message);
        unsafe { MessageBoxW(std::ptr::null_mut(), msg.as_ptr(), title.as_ptr(), MB_OK | MB_ICONINFORMATION) };
    }
}
