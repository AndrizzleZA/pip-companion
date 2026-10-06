//! Thin wrappers over the Win32 calls the shell needs. Nothing in here keeps state.

use std::ffi::c_void;
use pip_core::settings::Voice;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Media::Audio::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::SystemInformation::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// This PC's name, so the other device can say where a change came from.
pub fn device_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "This PC".into())
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn tick_ms() -> u64 {
    unsafe { GetTickCount64() }
}

// ---------------------------------------------------------------------------------------------
// Displays / DPI
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IRect {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
}

impl IRect {
    pub fn xywh(x: i32, y: i32, w: i32, h: i32) -> Self {
        IRect { l: x, t: y, r: x + w, b: y + h }
    }
    pub fn w(&self) -> i32 {
        self.r - self.l
    }
    pub fn h(&self) -> i32 {
        self.b - self.t
    }
    pub fn union(&self, o: &IRect) -> IRect {
        IRect { l: self.l.min(o.l), t: self.t.min(o.t), r: self.r.max(o.r), b: self.b.max(o.b) }
    }
    pub fn offset(&self, dx: i32, dy: i32) -> IRect {
        IRect { l: self.l + dx, t: self.t + dy, r: self.r + dx, b: self.b + dy }
    }
}

fn from_rect(r: RECT) -> IRect {
    IRect { l: r.left, t: r.top, r: r.right, b: r.bottom }
}

pub struct MonitorInfo {
    pub work: IRect,
    pub dpi: u32,
}

/// Monitor containing (or nearest to) a point. `must_exist`: None if the point is on no monitor.
pub fn monitor_at(x: i32, y: i32, must_exist: bool) -> Option<MonitorInfo> {
    unsafe {
        let flag = if must_exist { MONITOR_DEFAULTTONULL } else { MONITOR_DEFAULTTONEAREST };
        let hmon = MonitorFromPoint(POINT { x, y }, flag);
        if hmon.is_null() {
            return None;
        }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut mi) == 0 {
            return None;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        if GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) != 0 {
            dx = 96;
        }
        Some(MonitorInfo { work: from_rect(mi.rcWork), dpi: dx })
    }
}

pub fn primary_monitor() -> MonitorInfo {
    monitor_at(0, 0, false).expect("a primary monitor")
}

pub fn set_dpi_awareness() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

// ---------------------------------------------------------------------------------------------
// User presence
// ---------------------------------------------------------------------------------------------

/// Milliseconds since the last keyboard/mouse input (system-wide).
pub fn idle_ms() -> u64 {
    unsafe {
        let mut li = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        if GetLastInputInfo(&mut li) == 0 {
            return 0;
        }
        (GetTickCount() as u32).wrapping_sub(li.dwTime) as u64
    }
}

/// True if Windows says the user is busy with a fullscreen app, game or presentation.
pub fn user_busy_fullscreen() -> bool {
    unsafe {
        let mut state: QUERY_USER_NOTIFICATION_STATE = 0;
        if SHQueryUserNotificationState(&mut state) != 0 {
            return false;
        }
        matches!(state, QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE)
    }
}

/// Registers an invisible app bar so Windows tells us (ABN_FULLSCREENAPP) when fullscreen apps
/// open/close — an event instead of polling. No screen space is reserved (we never ABM_SETPOS).
pub fn register_appbar(hwnd: HWND, callback_msg: u32) {
    unsafe {
        let mut abd: APPBARDATA = std::mem::zeroed();
        abd.cbSize = std::mem::size_of::<APPBARDATA>() as u32;
        abd.hWnd = hwnd;
        abd.uCallbackMessage = callback_msg;
        SHAppBarMessage(ABM_NEW, &mut abd);
    }
}

pub fn unregister_appbar(hwnd: HWND) {
    unsafe {
        let mut abd: APPBARDATA = std::mem::zeroed();
        abd.cbSize = std::mem::size_of::<APPBARDATA>() as u32;
        abd.hWnd = hwnd;
        SHAppBarMessage(ABM_REMOVE, &mut abd);
    }
}

// ---------------------------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------------------------

fn reg_dword(root: HKEY, path: &str, name: &str) -> Option<u32> {
    unsafe {
        let mut val: u32 = 0;
        let mut size = 4u32;
        let p = wide(path);
        let n = wide(name);
        let r = RegGetValueW(
            root,
            p.as_ptr(),
            n.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut val as *mut u32 as *mut c_void,
            &mut size,
        );
        (r == 0).then_some(val)
    }
}

/// Windows "app mode" is dark?
pub fn system_dark_mode() -> bool {
    reg_dword(
        HKEY_CURRENT_USER,
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
        "AppsUseLightTheme",
    )
    .map(|v| v == 0)
    .unwrap_or(false)
}

/// Opt this process into dark native menus when the OS is dark. Uses the long-standing uxtheme
/// ordinals (SetPreferredAppMode=135, FlushMenuThemes=136); silently skipped if unavailable.
pub fn allow_dark_menus(dark: bool) {
    unsafe {
        let lib = windows_sys::Win32::System::LibraryLoader::LoadLibraryW(wide("uxtheme.dll").as_ptr());
        if lib.is_null() {
            return;
        }
        let set = windows_sys::Win32::System::LibraryLoader::GetProcAddress(lib, 135 as *const u8);
        let flush = windows_sys::Win32::System::LibraryLoader::GetProcAddress(lib, 136 as *const u8);
        if let (Some(set), Some(flush)) = (set, flush) {
            let set: extern "system" fn(i32) -> i32 = std::mem::transmute(set);
            let flush: extern "system" fn() = std::mem::transmute(flush);
            set(if dark { 2 } else { 3 }); // ForceDark / ForceLight
            flush();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Autostart (HKCU Run key — no admin rights, no scheduled task, no service)
// ---------------------------------------------------------------------------------------------

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "Pip";

pub fn set_autostart(enable: bool) -> bool {
    let exe = std::env::current_exe().unwrap_or_default();
    set_autostart_to(&exe, enable)
}

/// Same, but for a specific copy of Pip (used right after installing).
pub fn set_autostart_to(exe: &std::path::Path, enable: bool) -> bool {
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, wide(RUN_KEY).as_ptr(), 0, KEY_SET_VALUE | KEY_QUERY_VALUE, &mut key)
            != 0
        {
            return false;
        }
        let name = wide(RUN_VALUE);
        let ok = if enable {
            let cmd = wide(&format!("\"{}\" --autostart", exe.display()));
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                cmd.as_ptr() as *const u8,
                (cmd.len() * 2) as u32,
            ) == 0
        } else {
            let r = RegDeleteValueW(key, name.as_ptr());
            r == 0 || r == ERROR_FILE_NOT_FOUND
        };
        RegCloseKey(key);
        ok
    }
}

// ---------------------------------------------------------------------------------------------
// Sound: a few notes synthesised into memory when first played (no files, no audio engine).
// ---------------------------------------------------------------------------------------------

pub struct Chime(Vec<u8>);

/// A voice is just a few notes and how quickly they fade. Keeping it to this means another one
/// costs a line, not a sound file.
struct Recipe {
    /// Seconds of audio.
    len: f32,
    /// How fast each note dies away; bigger is shorter.
    decay: f32,
    /// How much of the octave above is mixed in — the difference between woody and bell-like.
    harmonic: f32,
    /// (hertz, seconds from the start).
    notes: &'static [(f32, f32)],
}

fn recipe(voice: Voice) -> Recipe {
    match voice {
        // The original: A5 then E6.
        Voice::Chime => Recipe { len: 0.55, decay: 7.0, harmonic: 0.12, notes: &[(880.0, 0.0), (1318.5, 0.11)] },
        // One note, left to ring.
        Voice::Bell => Recipe { len: 0.95, decay: 3.2, harmonic: 0.3, notes: &[(1046.5, 0.0)] },
        // Two quick blips, almost spoken.
        Voice::Blip => Recipe { len: 0.26, decay: 20.0, harmonic: 0.05, notes: &[(659.3, 0.0), (880.0, 0.07)] },
        // Soft and falling, more of a hum than a ring.
        Voice::Coo => Recipe { len: 0.7, decay: 5.5, harmonic: 0.04, notes: &[(659.3, 0.0), (523.3, 0.13)] },
    }
}

impl Chime {
    pub fn new(voice: Voice) -> Self {
        const RATE: u32 = 22_050;
        let r = recipe(voice);
        let len = (RATE as f32 * r.len) as usize;
        let mut samples = vec![0i16; len];
        for &(freq, start) in r.notes {
            let s0 = (start * RATE as f32) as usize;
            for (i, s) in samples.iter_mut().enumerate().skip(s0) {
                let t = (i - s0) as f32 / RATE as f32;
                let env = (1.0 - (-t * 60.0).exp()) * (-t * r.decay).exp();
                let v = (t * freq * std::f32::consts::TAU).sin() * 0.6
                    + (t * freq * 2.0 * std::f32::consts::TAU).sin() * r.harmonic;
                *s = s.saturating_add((v * env * 5200.0) as i16);
            }
        }
        let data_len = (len * 2) as u32;
        let mut wav = Vec::with_capacity(44 + data_len as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&RATE.to_le_bytes());
        wav.extend_from_slice(&(RATE * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            wav.extend_from_slice(&s.to_le_bytes());
        }
        Chime(wav)
    }

    pub fn play(&self) {
        unsafe {
            PlaySoundW(self.0.as_ptr() as *const u16, std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Process diagnostics (read only when the About page is open)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub private_bytes: u64,
    pub working_set: u64,
    pub cpu_ms: u64,
    pub handles: u32,
}

pub fn process_usage() -> Usage {
    use windows_sys::Win32::System::ProcessStatus::*;
    unsafe {
        let proc = GetCurrentProcess();
        let mut pmc: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        GetProcessMemoryInfo(proc, &mut pmc as *mut _ as *mut PROCESS_MEMORY_COUNTERS, pmc.cb);
        let (mut c, mut e, mut k, mut u) = (zero_ft(), zero_ft(), zero_ft(), zero_ft());
        GetProcessTimes(proc, &mut c, &mut e, &mut k, &mut u);
        let mut handles = 0u32;
        GetProcessHandleCount(proc, &mut handles);
        Usage {
            private_bytes: pmc.PrivateUsage as u64,
            working_set: pmc.WorkingSetSize as u64,
            cpu_ms: (ft_u64(k) + ft_u64(u)) / 10_000,
            handles,
        }
    }
}

fn zero_ft() -> FILETIME {
    FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }
}
fn ft_u64(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

/// Milliseconds since this process was created (for startup-time measurement).
pub fn ms_since_process_start() -> u64 {
    unsafe {
        let (mut c, mut e, mut k, mut u) = (zero_ft(), zero_ft(), zero_ft(), zero_ft());
        GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        let mut now = zero_ft();
        GetSystemTimeAsFileTime(&mut now);
        ft_u64(now).saturating_sub(ft_u64(c)) / 10_000
    }
}

pub fn open_folder(path: &std::path::Path) {
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            wide("open").as_ptr(),
            wide(&path.display().to_string()).as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Single-instance guard. Returns false if another instance already holds the mutex.
pub fn acquire_single_instance(name: &str) -> bool {
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, wide(name).as_ptr());
        !(h.is_null() || GetLastError() == ERROR_ALREADY_EXISTS)
    }
}

/// Returns pages that are no longer being touched to the OS (they stay on the standby list and
/// fault back cheaply if needed). Called once after startup settles and after Settings closes —
/// never periodically.
pub fn trim_working_set() {
    unsafe {
        windows_sys::Win32::System::Memory::SetProcessWorkingSetSizeEx(GetCurrentProcess(), usize::MAX, usize::MAX, 0);
    }
}

/// Unicode text on the clipboard, if any.
pub fn clipboard_text() -> Option<String> {
    use windows_sys::Win32::System::DataExchange::*;
    use windows_sys::Win32::System::Memory::*;
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        // Another app may hold the clipboard for a moment; retry briefly instead of failing.
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(std::ptr::null_mut()) != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        if !opened {
            return None;
        }
        let mut out = None;
        let h = GetClipboardData(CF_UNICODETEXT);
        if !h.is_null() {
            let p = GlobalLock(h as _) as *const u16;
            if !p.is_null() {
                let mut len = 0;
                while *p.add(len) != 0 && len < 1 << 20 {
                    len += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
                GlobalUnlock(h as _);
            }
        }
        CloseClipboard();
        out
    }
}

/// Opens a web page in the user's default browser.
pub fn open_url(url: &str) {
    if std::env::var_os("PIP_NO_BROWSER").is_some() {
        return; // automated tests
    }
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), wide("open").as_ptr(), wide(url).as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}
