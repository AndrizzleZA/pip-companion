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

/// What the companion is saying, as against what it sounds like.
///
/// A phrase is a shape — rising, falling, two taps — and the chosen [`Voice`] supplies the timbre,
/// so every noise comes from the same creature. The alternative was a second palette of unrelated
/// sounds, which is how a program ends up with a cute chime and a stock beep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phrase {
    /// The voice exactly as chosen: the original two notes.
    Plain,
    /// Up a step and held: a question.
    Rising,
    /// Two short dry knocks.
    Knock,
    /// Three notes climbing: pleased.
    Trill,
    /// Down and softening: end of the day.
    Falling,
}

/// Multipliers on the chosen voice's notes, plus how the phrase bends and how long it runs.
///
/// Ratios rather than fixed hertz, so a phrase keeps the pitch of whichever voice is set: a Blip
/// trill and a Bell trill are the same tune in two different instruments.
fn phrase(p: Phrase) -> (&'static [(f32, f32)], f32, f32) {
    match p {
        Phrase::Plain => (&[(1.0, 0.0)], 1.0, 0.0),
        Phrase::Rising => (&[(1.0, 0.0), (1.26, 0.10)], 1.0, 2.5),
        Phrase::Knock => (&[(0.72, 0.0), (0.72, 0.085)], 0.55, -1.0),
        Phrase::Trill => (&[(1.0, 0.0), (1.19, 0.07), (1.5, 0.14)], 1.15, 1.5),
        Phrase::Falling => (&[(1.0, 0.0), (0.84, 0.12), (0.67, 0.26)], 1.5, -2.0),
    }
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
        Chime::say(voice, Phrase::Plain, 1.0, 0)
    }

    /// Builds one utterance: a phrase in a voice, at a volume, with a per-play wobble.
    ///
    /// `seed` detunes the whole thing by up to about a semitone and a half. It is the difference
    /// between a creature and a doorbell — the same noise to the exact hertz every single time is
    /// the thing that makes a sound effect wear out, and it costs one multiply.
    ///
    /// Built when it is played and dropped when the next one is built: the buffer only has to
    /// outlive `PlaySound`, so a shelf of cached sounds would be a few hundred kilobytes held for
    /// nothing. Synthesising half a second of 22 kHz audio is a few thousand multiplies.
    pub fn say(voice: Voice, ph: Phrase, volume: f32, seed: u64) -> Self {
        const RATE: u32 = 22_050;
        let r = recipe(voice);
        let (steps, len_scale, bend) = phrase(ph);
        // A cheap, stable spread in roughly 0.96..1.04.
        let wobble = 1.0 + ((seed % 101) as f32 / 100.0 - 0.5) * 0.08;
        let base = r.notes.first().map(|n| n.0).unwrap_or(880.0) * wobble;
        let len = (RATE as f32 * r.len * len_scale) as usize;
        let mut samples = vec![0i16; len.max(1)];
        let amp = 5200.0 * volume.clamp(0.0, 1.0);
        for &(ratio, start) in steps {
            let freq = base * ratio;
            let s0 = (start * RATE as f32) as usize;
            for (i, s) in samples.iter_mut().enumerate().skip(s0) {
                let t = (i - s0) as f32 / RATE as f32;
                let env = (1.0 - (-t * 60.0).exp()) * (-t * r.decay).exp();
                // The glide: the note slides as it sounds, which is most of what separates a
                // voice from a tone. Phase is the integral of frequency, not frequency × time.
                let phase = (freq * t + bend * 0.03 * freq * t * t) * std::f32::consts::TAU;
                let v = phase.sin() * 0.6 + (phase * 2.0).sin() * r.harmonic;
                *s = s.saturating_add((v * env * amp) as i16);
            }
        }
        // Normalise, so that choosing a voice is choosing a character and not a volume.
        //
        // The recipes set their own decay, and a fast one spends most of its energy before the
        // attack has finished rising: Blip peaked at about a fifteenth of full scale where Bell
        // reached a third, so picking it quietly turned the sound down by four times. Found by
        // the test below rather than by ear, having been that way since the voices were added.
        let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        if peak > 0 {
            let want = 18_000.0 * volume.clamp(0.0, 1.0);
            let gain = want / peak as f32;
            for s in samples.iter_mut() {
                *s = (*s as f32 * gain).clamp(-32_000.0, 32_000.0) as i16;
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

/// Which Windows this is, for a crash report that arrives from someone else's machine.
///
/// Read from the registry rather than `GetVersionEx`, which reports 6.2 for everything after 8.1
/// unless the executable carries a compatibility manifest saying otherwise. Read once at startup,
/// never from the panic hook, which must not be doing registry work.
pub fn os_build() -> String {
    let path = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let read = |name: &str| -> Option<String> {
        let n = wide(name);
        let mut buf = [0u16; 128];
        let mut len = (buf.len() * 2) as u32;
        let ok = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                path.as_ptr(),
                n.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut c_void,
                &mut len,
            )
        };
        if ok != ERROR_SUCCESS {
            return None;
        }
        let chars = (len as usize / 2).saturating_sub(1);
        Some(String::from_utf16_lossy(&buf[..chars.min(buf.len())]))
    };
    let build = read("CurrentBuild").unwrap_or_default();
    // Windows 11 still calls itself 10 in ProductName; the build number is the only honest answer.
    let major = if build.parse::<u32>().unwrap_or(0) >= 22000 { "11" } else { "10" };
    let display = read("DisplayVersion").unwrap_or_default();
    format!("Windows {major} {display} (build {build})").replace("  ", " ")
}

#[cfg(test)]
mod the_voice {
    use super::*;

    /// Reads the samples back out of the WAV this built.
    fn samples(c: &Chime) -> Vec<i16> {
        c.0[44..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
    }

    /// Whether it *sounds* nice is not something a test can tell me, and I cannot hear it either.
    /// What a test can tell me is that every combination produces real audio — that no phrase is
    /// silent, none is clipped into a buzz, and none comes out the wrong length. Those are the
    /// three ways a synthesised sound goes wrong without anybody noticing until a user does.
    #[test]
    fn every_phrase_in_every_voice_is_real_audio() {
        let phrases = [Phrase::Plain, Phrase::Rising, Phrase::Knock, Phrase::Trill, Phrase::Falling];
        for voice in Voice::ALL {
            for ph in phrases {
                let c = Chime::say(voice, ph, 1.0, 12_345);
                let s = samples(&c);
                assert!(s.len() > 2_000, "{voice:?}/{ph:?} is only {} samples long", s.len());
                let peak = s.iter().map(|v| v.unsigned_abs()).max().unwrap_or(0);
                assert!(peak > 15_000, "{voice:?}/{ph:?} is inaudible (peak {peak})");
                assert!(peak < 20_000, "{voice:?}/{ph:?} is louder than the rest (peak {peak})");
                let clipped = s.iter().filter(|v| v.unsigned_abs() > 32_000).count();
                assert!(clipped == 0, "{voice:?}/{ph:?} clips on {clipped} samples, which is a buzz");
            }
        }
    }

    /// Volume has to actually be a volume, since the setting rides on it.
    #[test]
    fn volume_scales_the_sound() {
        let loud = samples(&Chime::say(Voice::Chime, Phrase::Plain, 1.0, 1)).iter().map(|v| v.unsigned_abs()).max().unwrap();
        let soft = samples(&Chime::say(Voice::Chime, Phrase::Plain, 0.3, 1)).iter().map(|v| v.unsigned_abs()).max().unwrap();
        assert!(soft * 2 < loud, "quiet ({soft}) is not meaningfully quieter than loud ({loud})");
        let off = samples(&Chime::say(Voice::Chime, Phrase::Plain, 0.0, 1)).iter().map(|v| v.unsigned_abs()).max().unwrap();
        assert_eq!(off, 0, "zero volume should be silence");
    }

    /// The same noise to the exact sample every time is what makes a sound effect wear out.
    #[test]
    fn the_same_phrase_is_never_quite_the_same_twice() {
        let a = samples(&Chime::say(Voice::Coo, Phrase::Rising, 1.0, 7));
        let b = samples(&Chime::say(Voice::Coo, Phrase::Rising, 1.0, 64));
        assert_ne!(a, b, "two plays of the same phrase came out byte-identical");
    }
}
