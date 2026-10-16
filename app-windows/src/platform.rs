//! Thin wrappers over the Win32 calls the shell needs. Nothing in here keeps state.

use std::ffi::c_void;
use pip_core::settings::{Character, Voice};
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
    /// How far the pitch wavers, as a fraction. This is the single biggest reason a tone sounds
    /// like a throat rather than a struck object: nothing alive holds a pitch perfectly still.
    vibrato: f32,
    /// How fast it wavers, in hertz. Around 5–7 is a voice; faster is an insect, slower is a wobble.
    vibrato_hz: f32,
    /// A puff of noise under the attack. Tiny amounts read as breath; any more reads as a hiss.
    breath: f32,
    /// How quickly the note arrives. Slower is sung, faster is struck.
    attack: f32,
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
    /// Two very short rising squeaks. Surprise, or something noticed.
    Chirp,
    /// Three quick low notes, swallowed. Water.
    Burble,
    /// Low, soft and slow — the nearest thing to contentment this can make.
    Purr,
    /// Up and a long way back down. Waking, or giving in to bedtime.
    Yawn,
    /// Falling, wavering, unsure. For when something did not go to plan.
    Whine,
    /// A single soft touch: the shortest, quietest thing it can say.
    Touch,
    /// Four notes climbing. The biggest noise it makes, for when something is finished.
    Cheer,
}

/// Multipliers on the chosen voice: which notes, how long it runs, how it bends, and how fast it
/// dies away. The decay multiplier is what lets a click stay a click in a voice built to ring �
/// without it the touch sound was 57 ms in Blip and half a second in Bell, which is two different
/// sounds wearing one name.
///
/// Ratios rather than fixed hertz, so a phrase keeps the pitch of whichever voice is set: a Blip
/// trill and a Bell trill are the same tune in two different instruments.
fn phrase(p: Phrase) -> (&'static [(f32, f32)], f32, f32, f32) {
    match p {
        Phrase::Plain => (&[(1.0, 0.0)], 1.0, 0.0, 1.0),
        Phrase::Rising => (&[(1.0, 0.0), (1.26, 0.10)], 1.0, 2.5, 1.0),
        Phrase::Knock => (&[(0.72, 0.0), (0.72, 0.085)], 0.55, -1.0, 1.0),
        Phrase::Trill => (&[(1.0, 0.0), (1.19, 0.07), (1.5, 0.14)], 1.15, 1.5, 1.0),
        Phrase::Falling => (&[(1.0, 0.0), (0.84, 0.12), (0.67, 0.26)], 1.5, -2.0, 1.0),
        // Small and quick and up: the shape of a surprised noise.
        Phrase::Chirp => (&[(1.35, 0.0), (1.6, 0.06)], 0.5, 5.0, 1.0),
        // Low and tumbling, three of them, like something being swallowed.
        Phrase::Burble => (&[(0.78, 0.0), (0.7, 0.06), (0.62, 0.12)], 0.7, -3.0, 1.0),
        // Barely moving. The slow waver of the voice does most of the work here.
        Phrase::Purr => (&[(0.52, 0.0), (0.52, 0.18)], 1.6, -0.5, 1.0),
        // Up, and a long way back down: the whole shape of a yawn is the descent.
        Phrase::Yawn => (&[(0.85, 0.0), (1.15, 0.14), (0.62, 0.34)], 2.2, -4.0, 1.0),
        // Falling and unsure. Deliberately the only unhappy noise, and it is never aimed at
        // anybody — it belongs to the companion tripping over, not to anything the person did.
        Phrase::Whine => (&[(1.1, 0.0), (0.95, 0.10), (0.8, 0.22)], 1.3, -6.0, 1.0),
        // One short breathy note, barely pitched. Its own sound and nothing else uses it: a click
        // happens dozens of times a day, so it has to be the least of all of these.
        Phrase::Touch => (&[(0.58, 0.0)], 0.62, -1.5, 2.8),
        // Four notes climbing, the top one held: the biggest thing it says.
        Phrase::Cheer => (&[(1.0, 0.0), (1.19, 0.06), (1.5, 0.12), (1.78, 0.19)], 1.5, 2.0, 1.0),
    }
}

/// Each companion's own voice.
///
/// Seven timbres, one per character, chosen to match the thing you are looking at: the big round
/// one is low and slow, the smallest is high and quick, the mushroom is woody and short. It is the
/// same bargain as the drawing � a creature is eight numbers here, so the eighth creature costs a
/// line rather than a folder of recordings.
///
/// Changing companion changes voice at once, because nothing is cached: the sound is built from
/// whoever is on screen at the moment it plays.
fn creature_recipe(who: Character) -> Recipe {
    match who {
        // Green and growing: bright, with a light waver, like something young.
        Character::Sprout => Recipe {
            len: 0.5,
            decay: 7.5,
            harmonic: 0.11,
            notes: &[(880.0, 0.0), (1174.7, 0.10)],
            vibrato: 0.016,
            vibrato_hz: 6.4,
            breath: 0.06,
            attack: 42.0,
        },
        // Soft, quick and breathy, pitched high. The ears are the giveaway and so is this.
        Character::Bunny => Recipe {
            len: 0.42,
            decay: 10.0,
            harmonic: 0.07,
            notes: &[(987.8, 0.0), (1318.5, 0.08)],
            vibrato: 0.024,
            vibrato_hz: 7.6,
            breath: 0.14,
            attack: 60.0,
        },
        // Woody and short, with almost no ring: a mushroom is not a bell.
        Character::Kit => Recipe {
            len: 0.38,
            decay: 13.0,
            harmonic: 0.22,
            notes: &[(523.3, 0.0), (659.3, 0.09)],
            vibrato: 0.012,
            vibrato_hz: 5.2,
            breath: 0.08,
            attack: 90.0,
        },
        // Airy and feathered, a fast small flutter.
        Character::Tuft => Recipe {
            len: 0.46,
            decay: 9.0,
            harmonic: 0.09,
            notes: &[(783.99, 0.0), (1046.5, 0.09)],
            vibrato: 0.027,
            vibrato_hz: 8.2,
            breath: 0.11,
            attack: 55.0,
        },
        // The widest body in the cast, and the lowest, slowest, least hurried voice.
        Character::Pebble => Recipe {
            len: 0.85,
            decay: 4.0,
            harmonic: 0.17,
            notes: &[(392.0, 0.0), (329.6, 0.16)],
            vibrato: 0.009,
            vibrato_hz: 4.2,
            breath: 0.07,
            attack: 18.0,
        },
        // Sweet and ringing, with a flutter on the end, like the petals.
        Character::Bloom => Recipe {
            len: 0.72,
            decay: 5.0,
            harmonic: 0.26,
            notes: &[(1046.5, 0.0), (1318.5, 0.11)],
            vibrato: 0.020,
            vibrato_hz: 6.8,
            breath: 0.05,
            attack: 34.0,
        },
        // The smallest of them: very high, very quick, very quiet, and papery.
        Character::Moth => Recipe {
            len: 0.3,
            decay: 16.0,
            harmonic: 0.04,
            notes: &[(1318.5, 0.0), (1567.98, 0.06)],
            vibrato: 0.034,
            vibrato_hz: 10.5,
            breath: 0.18,
            attack: 110.0,
        },
    }
}

fn recipe(voice: Voice) -> Recipe {
    match voice {
        // Resolved by the caller, which is the only place that knows who the companion is.
        Voice::Own => creature_recipe(Character::Sprout),
        // The original: A5 then E6. Still the brightest, now with a throat behind it.
        Voice::Chime => Recipe {
            len: 0.55,
            decay: 7.0,
            harmonic: 0.12,
            notes: &[(880.0, 0.0), (1318.5, 0.11)],
            vibrato: 0.013,
            vibrato_hz: 6.2,
            breath: 0.05,
            attack: 45.0,
        },
        // One note, left to ring. The least animal of the four and meant to stay that way.
        Voice::Bell => Recipe {
            len: 0.95,
            decay: 3.2,
            harmonic: 0.3,
            notes: &[(1046.5, 0.0)],
            vibrato: 0.006,
            vibrato_hz: 5.0,
            breath: 0.02,
            attack: 70.0,
        },
        // Quick and chattery: a fast flutter and a hard attack, like something very small.
        Voice::Blip => Recipe {
            len: 0.26,
            decay: 20.0,
            harmonic: 0.05,
            notes: &[(659.3, 0.0), (880.0, 0.07)],
            vibrato: 0.022,
            vibrato_hz: 9.5,
            breath: 0.09,
            attack: 120.0,
        },
        // The most creature of them: deep waver, soft arrival, audible breath.
        Voice::Coo => Recipe {
            len: 0.7,
            decay: 5.5,
            harmonic: 0.04,
            notes: &[(659.3, 0.0), (523.3, 0.13)],
            vibrato: 0.030,
            vibrato_hz: 5.4,
            breath: 0.13,
            attack: 22.0,
        },
    }
}

impl Chime {
    pub fn new(voice: Voice, who: Character) -> Self {
        Chime::say(voice, who, Phrase::Plain, 1.0, 0)
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
    pub fn say(voice: Voice, who: Character, ph: Phrase, volume: f32, seed: u64) -> Self {
        const RATE: u32 = 22_050;
        // "Its own" means the companion on screen right now, which is why this is resolved here
        // and not stored: changing character changes the voice with the next sound it makes.
        let r = if voice == Voice::Own { creature_recipe(who) } else { recipe(voice) };
        let (steps, len_scale, bend, decay_mul) = phrase(ph);
        // A cheap, stable spread in roughly 0.96..1.04.
        let wobble = 1.0 + ((seed % 101) as f32 / 100.0 - 0.5) * 0.08;
        let base = r.notes.first().map(|n| n.0).unwrap_or(880.0) * wobble;
        let len = (RATE as f32 * r.len * len_scale) as usize;
        let mut samples = vec![0i16; len.max(1)];
        let amp = 5200.0 * volume.clamp(0.0, 1.0);
        let tau = std::f32::consts::TAU;
        // Noise for the breath, from the same seed, so a sound is still the same sound twice.
        let mut rng = seed | 1;
        let mut noise = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng >> 40) as f32 / 8_388_608.0 - 1.0
        };
        for &(ratio, start) in steps {
            let freq = base * ratio;
            let s0 = (start * RATE as f32) as usize;
            // Phase is accumulated rather than computed from `t`, which is what lets the pitch
            // move freely: waver it, bend it, do both, and the waveform stays continuous. Working
            // it out as frequency × time cannot do that without clicking every time the pitch
            // changes, and a click is the one thing that makes a synthesised voice sound synthetic.
            let mut phase = 0.0f32;
            for i in s0..samples.len() {
                let t = (i - s0) as f32 / RATE as f32;
                let env = (1.0 - (-t * r.attack).exp()) * (-t * r.decay * decay_mul).exp();

                // Nothing alive holds a pitch still. The waver is the single biggest reason this
                // reads as a throat and not a struck object.
                let waver = 1.0 + r.vibrato * (tau * r.vibrato_hz * t).sin();
                // And the note itself slides while it sounds, settling as it goes.
                let glide = 1.0 + bend * 0.07 * (1.0 - (-t * 7.0).exp());
                phase += tau * freq * waver * glide / RATE as f32;

                // A small stack of partials rather than a pure tone. The detuned one is what
                // gives it a body: two things an almost-identical distance apart beat against
                // each other slowly, which is what a voice does and a sine wave does not.
                let v = phase.sin() * 0.62
                    + (phase * 2.0).sin() * r.harmonic
                    + (phase * 3.0).sin() * r.harmonic * 0.4
                    + (phase * 2.01).sin() * r.harmonic * 0.35;

                // Breath, only under the attack, gone almost at once.
                let b = noise() * r.breath * (-t * 26.0).exp();

                samples[i] = samples[i].saturating_add(((v + b) * env * amp) as i16);
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
        use pip_core::settings::Character;
        let phrases = [
            Phrase::Plain, Phrase::Rising, Phrase::Knock, Phrase::Trill, Phrase::Falling,
            Phrase::Chirp, Phrase::Burble, Phrase::Purr, Phrase::Yawn, Phrase::Whine, Phrase::Cheer,
            Phrase::Touch,
        ];
        for voice in Voice::ALL {
            for ph in phrases {
                for who in Character::ALL {
                let c = Chime::say(voice, who, ph, 1.0, 12_345);
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
    }

    /// Volume has to actually be a volume, since the setting rides on it.
    #[test]
    fn volume_scales_the_sound() {
        let loud = samples(&Chime::say(Voice::Chime, pip_core::settings::Character::Sprout, Phrase::Plain, 1.0, 1)).iter().map(|v| v.unsigned_abs()).max().unwrap();
        let soft = samples(&Chime::say(Voice::Chime, pip_core::settings::Character::Sprout, Phrase::Plain, 0.3, 1)).iter().map(|v| v.unsigned_abs()).max().unwrap();
        assert!(soft * 2 < loud, "quiet ({soft}) is not meaningfully quieter than loud ({loud})");
        let off = samples(&Chime::say(Voice::Chime, pip_core::settings::Character::Sprout, Phrase::Plain, 0.0, 1)).iter().map(|v| v.unsigned_abs()).max().unwrap();
        assert_eq!(off, 0, "zero volume should be silence");
    }

    /// The same noise to the exact sample every time is what makes a sound effect wear out.
    #[test]
    fn the_same_phrase_is_never_quite_the_same_twice() {
        let a = samples(&Chime::say(Voice::Coo, pip_core::settings::Character::Sprout, Phrase::Rising, 1.0, 7));
        let b = samples(&Chime::say(Voice::Coo, pip_core::settings::Character::Sprout, Phrase::Rising, 1.0, 64));
        assert_ne!(a, b, "two plays of the same phrase came out byte-identical");
    }
}

#[cfg(test)]
mod each_creature_its_own {
    use super::*;
    use pip_core::settings::Character;

    fn samples(c: &Chime) -> Vec<i16> {
        c.0[44..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
    }

    /// Seven companions, seven voices. A character that sounded like another would be a table
    /// entry copied and not edited, which is exactly the kind of mistake nobody hears until they
    /// have both companions and wonder why changing made no difference.
    #[test]
    fn no_two_companions_sound_alike() {
        let mut seen: Vec<(Character, Vec<i16>)> = Vec::new();
        for who in Character::ALL {
            let s = samples(&Chime::say(Voice::Own, who, Phrase::Plain, 1.0, 99));
            for (other, prev) in &seen {
                assert_ne!(&s, prev, "{who:?} and {other:?} make exactly the same noise");
                // Length alone separates most of them, and where it does not the waveform must.
                assert!(
                    s.len() != prev.len() || s != *prev,
                    "{who:?} and {other:?} are indistinguishable"
                );
            }
            seen.push((who, s));
        }
    }

    /// And the named voices stay put: picking Bell gets Bell whoever is on screen, because it was
    /// chosen for being a bell rather than for being the companion.
    #[test]
    fn a_named_voice_ignores_the_companion() {
        for voice in [Voice::Chime, Voice::Bell, Voice::Blip, Voice::Coo] {
            let a = samples(&Chime::say(voice, Character::Moth, Phrase::Plain, 1.0, 5));
            let b = samples(&Chime::say(voice, Character::Pebble, Phrase::Plain, 1.0, 5));
            assert_eq!(a, b, "{voice:?} changed when the companion did");
        }
    }

    /// The companion's own voice must follow the companion, which is the whole point of it.
    #[test]
    fn its_own_voice_follows_the_companion() {
        let a = samples(&Chime::say(Voice::Own, Character::Moth, Phrase::Plain, 1.0, 5));
        let b = samples(&Chime::say(Voice::Own, Character::Pebble, Phrase::Plain, 1.0, 5));
        assert_ne!(a, b, "the smallest and the largest companion sound identical");
    }
}
