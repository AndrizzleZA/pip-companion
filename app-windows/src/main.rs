//! Pip for Windows — entry point and Win32 plumbing.
//!
//! One process, one UI thread, one message loop. The controller (`App`) lives in a thread-local
//! `RefCell`; window procedures borrow it per message. Operations that run nested message loops
//! (menus, creating/destroying windows) are queued as `Effect`s and executed *after* the borrow is
//! released, so re-entrant messages can always reach the app safely.

#![windows_subsystem = "windows"]

mod behaviour;
mod app;
mod crash;
mod install;
mod calsync;
mod drivesync;
mod mailsync;
mod oauth;
mod net;
mod secrets;
mod pet;
mod platform;
mod render;
mod settings_ui;
mod storage;
mod surface;
mod tray;

use app::{App, Effect, WM_APPBAR, WM_TRAY};
use platform::wide;
use std::cell::RefCell;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const PET_CLASS: &str = "PipCompanionPet";
const SETTINGS_CLASS: &str = "PipCompanionSettings";
const COPYDATA_MAGIC: usize = 0x5049_5031; // "PIP1"
const NIN_KEYSELECT: u32 = NIN_SELECT | 1; // NIN_SELECT | NINF_KEY
const WM_MOUSEHOVER: u32 = 0x02A1;
const WM_MOUSELEAVE: u32 = 0x02A3;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static TASKBAR_CREATED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Runs `f` with the app if it is not already borrowed (re-entrant messages fall through).
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().map(f),
        Err(_) => None,
    })
}

fn run_effects() {
    loop {
        let Some(effects) = with_app(|a| std::mem::take(&mut a.effects)) else { return };
        if effects.is_empty() {
            return;
        }
        for e in effects {
            match e {
                Effect::Menu(x, y) => show_menu(x, y),
                Effect::OpenSettings => open_settings(),
                Effect::Install => match install::install(true) {
                    Ok(exe) => {
                        // Hand over to the installed copy, then let this one go.
                        let _ = std::process::Command::new(&exe).spawn();
                        with_app(|a| a.shutdown());
                        unsafe { PostQuitMessage(0) };
                    }
                    Err(e) => unsafe {
                        MessageBoxW(std::ptr::null_mut(), wide(&e).as_ptr(), wide("Pip").as_ptr(), MB_OK | MB_ICONWARNING);
                    },
                },
                Effect::Uninstall => {
                    // The dialog is modal; if the user says no, Pip simply carries on.
                    let text = wide("Remove Pip from this PC?

Your settings and connections will be deleted too.");
                    let yes = unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), wide("Pip").as_ptr(), MB_YESNO | MB_ICONQUESTION) } == IDYES;
                    if yes {
                        install::uninstall(false);
                        with_app(|a| a.shutdown());
                        unsafe { PostQuitMessage(0) };
                    }
                }
                Effect::CloseSettings => {
                    if let Some(h) = with_app(|a| a.ui.as_ref().map(|u| u.hwnd)).flatten() {
                        unsafe { DestroyWindow(h) };
                    }
                }
                Effect::Quit => {
                    if let Some(h) = with_app(|a| a.ui.as_ref().map(|u| u.hwnd)).flatten() {
                        unsafe { DestroyWindow(h) };
                    }
                    with_app(|a| a.shutdown());
                    unsafe { PostQuitMessage(0) };
                }
            }
        }
    }
}

fn show_menu(x: i32, y: i32) {
    let Some((hwnd, menu)) = with_app(|a| (a.hwnd, a.build_menu())) else { return };
    unsafe {
        SetForegroundWindow(hwnd); // required so the menu closes when clicking elsewhere
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_RIGHTALIGN, x, y, 0, hwnd, std::ptr::null());
        DestroyMenu(menu);
        PostMessageW(hwnd, WM_NULL, 0, 0);
        if cmd != 0 {
            with_app(|a| a.menu_command(cmd as u32));
        }
    }
    run_effects();
}

fn open_settings() {
    if let Some(Some(h)) = with_app(|a| a.ui.as_ref().map(|u| u.hwnd)) {
        unsafe {
            ShowWindow(h, SW_RESTORE);
            SetForegroundWindow(h);
        }
        return;
    }
    let Some((anchor, name, body, who)) =
        with_app(|a| {
            (a.pet.anchor, a.settings.pet.name.clone(), a.settings.pet.color.rgb(), a.settings.pet.character.shown(a.state.experience))
        })
    else {
        return;
    };
    let mon = platform::monitor_at(anchor.0 + 20, anchor.1 + 20, false).unwrap_or_else(platform::primary_monitor);
    let s = mon.dpi as f32 / 96.0;
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    let mut rc = RECT { left: 0, top: 0, right: (settings_ui::LOGICAL_W as f32 * s) as i32, bottom: (settings_ui::LOGICAL_H as f32 * s) as i32 };
    unsafe { windows_sys::Win32::UI::HiDpi::AdjustWindowRectExForDpi(&mut rc, style, 0, 0, mon.dpi) };
    let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
    let x = mon.work.l + (mon.work.w() - w) / 2;
    let y = mon.work.t + (mon.work.h() - h) / 2;
    let title = wide(&format!("{name} — Settings"));
    let hwnd = unsafe {
        CreateWindowExW(0, wide(SETTINGS_CLASS).as_ptr(), title.as_ptr(), style, x, y, w, h, std::ptr::null_mut(), std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), std::ptr::null())
    };
    if hwnd.is_null() {
        return;
    }
    settings_ui::set_window_icon(hwnd, body, who);
    with_app(|a| {
        a.ui_opened(hwnd);
        a.ui_apply_chrome();
    });
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
}

fn lparam_xy(lp: LPARAM) -> (i32, i32) {
    ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32)
}

unsafe extern "system" fn pet_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let handled: Option<LRESULT> = match msg {
        WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
        WM_TIMER => with_app(|a| a.on_timer(wp)).map(|_| 0),
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_xy(lp);
            with_app(|a| a.mouse_down(x, y)).map(|_| 0)
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_xy(lp);
            with_app(|a| a.mouse_up(x, y)).map(|_| 0)
        }
        WM_LBUTTONDBLCLK => {
            let (x, y) = lparam_xy(lp);
            with_app(|a| a.double_click(x, y)).map(|_| 0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lp);
            with_app(|a| {
                if !a.pet.mouse_inside {
                    a.pet.mouse_inside = true;
                    let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_HOVER, hwndTrack: hwnd, dwHoverTime: 450 };
                    unsafe { TrackMouseEvent(&mut tme) };
                }
                a.mouse_move(x, y);
            })
            .map(|_| 0)
        }
        WM_MOUSEHOVER => {
            let (x, y) = lparam_xy(lp);
            with_app(|a| a.mouse_hover(x, y)).map(|_| 0)
        }
        WM_MOUSELEAVE => with_app(|a| a.mouse_leave()).map(|_| 0),
        WM_SETCURSOR => {
            let (x, y) = platform::cursor_pos();
            let over_button = with_app(|a| a.pet.hit_button(x - a.pet.win.l, y - a.pet.win.t).is_some()).unwrap_or(false);
            SetCursor(LoadCursorW(std::ptr::null_mut(), if over_button { IDC_HAND } else { IDC_ARROW }));
            Some(1)
        }
        WM_RBUTTONUP | WM_CONTEXTMENU => {
            let (x, y) = platform::cursor_pos();
            with_app(|a| a.effects.push(Effect::Menu(x, y))).map(|_| 0)
        }
        WM_TRAY => {
            // NOTIFYICON_VERSION_4: event in LOWORD(lParam)
            match (lp & 0xffff) as u32 {
                // v4 sends both raw mouse messages and NIN_*; react only to the semantic ones.
                NIN_SELECT | NIN_KEYSELECT => with_app(|a| {
                    let v = !a.pet.visible;
                    a.set_visible(v);
                }),
                WM_CONTEXTMENU => {
                    let (x, y) = platform::cursor_pos();
                    with_app(|a| a.effects.push(Effect::Menu(x, y)))
                }
                NIN_BALLOONUSERCLICK => with_app(|a| {
                    a.set_visible(true);
                    a.click_creature();
                }),
                _ => None,
            };
            Some(0)
        }
        WM_APPBAR => {
            if wp as u32 == ABN_FULLSCREENAPP {
                with_app(|a| a.on_fullscreen(lp != 0));
            }
            Some(0)
        }
        WM_WTSSESSION_CHANGE => {
            match wp as u32 {
                0x7 => with_app(|a| a.on_session(true)),  // WTS_SESSION_LOCK
                0x8 => with_app(|a| a.on_session(false)), // WTS_SESSION_UNLOCK
                _ => None,
            };
            Some(0)
        }
        WM_POWERBROADCAST => {
            if matches!(wp as u32, PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND) {
                with_app(|a| a.on_resume());
            }
            Some(1)
        }
        WM_TIMECHANGE => with_app(|a| a.reschedule()).map(|_| 0),
        WM_DISPLAYCHANGE | WM_DPICHANGED => with_app(|a| a.on_display_change()).map(|_| 0),
        WM_SETTINGCHANGE => {
            let is_theme = lp != 0 && {
                let p = lp as *const u16;
                let s: Vec<u16> = (0..64).map(|i| *p.add(i)).take_while(|&c| c != 0).collect();
                String::from_utf16_lossy(&s) == "ImmersiveColorSet"
            };
            if is_theme {
                with_app(|a| a.on_theme_change());
            }
            Some(0)
        }
        mailsync::WM_MAILSYNC => {
            let result = mailsync::take_result(lp);
            with_app(move |a| a.on_mailsync(result));
            Some(0)
        }
        oauth::WM_SIGNIN => {
            let result = oauth::take_result(lp);
            with_app(move |a| a.on_signin(result));
            Some(0)
        }
        drivesync::WM_DRIVESYNC => {
            let result = drivesync::take_result(lp);
            with_app(move |a| a.on_drivesync(result));
            Some(0)
        }
        calsync::WM_CALSYNC => {
            // Take ownership first so the payload is freed even if the app is busy.
            let result = calsync::take_result(lp);
            with_app(move |a| a.on_calsync(result));
            Some(0)
        }
        WM_COPYDATA => {
            let cds = &*(lp as *const COPYDATASTRUCT);
            if cds.dwData == COPYDATA_MAGIC && !cds.lpData.is_null() {
                let bytes = std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize);
                let cmd = String::from_utf8_lossy(bytes).to_string();
                let reply = with_app(|a| a.command(&cmd)).unwrap_or_else(|| "busy".into());
                let _ = std::fs::write(storage::Storage::open().dir().join("cmd-reply.txt"), reply);
                Some(1)
            } else {
                Some(0)
            }
        }
        WM_CLOSE => Some(0), // only Quit ends the app
        WM_DESTROY => {
            PostQuitMessage(0);
            Some(0)
        }
        _ if msg == TASKBAR_CREATED.with(|t| t.get()) && msg != 0 => with_app(|a| a.on_taskbar_created()).map(|_| 0),
        _ => None,
    };
    run_effects();
    match handled {
        Some(r) => r,
        None => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn settings_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let handled: Option<LRESULT> = match msg {
        WM_PAINT => with_app(|a| a.ui_paint()).map(|_| 0),
        WM_ERASEBKGND => Some(1),
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_xy(lp);
            with_app(|a| a.ui_click(x as f32, y as f32)).map(|_| 0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lp);
            with_app(|a| a.ui_mouse_move(x as f32, y as f32)).map(|_| 0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp >> 16) & 0xffff) as i16 as i32;
            with_app(|a| a.ui_wheel(delta)).map(|_| 0)
        }
        WM_SETCURSOR if (lp & 0xffff) as u32 == HTCLIENT => {
            let (sx, sy) = platform::cursor_pos();
            let mut p = POINT { x: sx, y: sy };
            windows_sys::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut p);
            let cur = with_app(|a| a.ui_cursor(p.x as f32, p.y as f32)).flatten().unwrap_or(IDC_ARROW);
            SetCursor(LoadCursorW(std::ptr::null_mut(), cur));
            Some(1)
        }
        WM_CHAR => with_app(|a| a.ui_char(wp as u32)).map(|_| 0),
        // Keyboard reach: Tab, arrows, Enter and Space, handled by the settings window itself.
        WM_KEYDOWN if wp as u16 != VK_ESCAPE => {
            let shift = (GetKeyState(0x10) as u16 & 0x8000) != 0;
            match with_app(|a| a.ui_key(wp as u16, shift)) {
                Some(true) => Some(0),
                _ => None,
            }
        }
        WM_KEYDOWN if wp as u16 == VK_ESCAPE => {
            // Esc closes the window unless it is ending a text edit.
            if with_app(|a| a.ui.as_ref().is_some_and(|u| u.focus.is_none())) == Some(true) {
                DestroyWindow(hwnd);
            }
            Some(0)
        }
        WM_TIMER => with_app(|a| a.ui_usage_tick()).map(|_| 0),
        WM_DPICHANGED => {
            let r = &*(lp as *const RECT);
            SetWindowPos(hwnd, std::ptr::null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            InvalidateRect(hwnd, std::ptr::null(), 0);
            Some(0)
        }
        WM_SETTINGCHANGE => {
            with_app(|a| {
                a.ui_apply_chrome();
                a.ui_invalidate();
            });
            None
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            Some(0)
        }
        WM_DESTROY => with_app(|a| a.ui_destroyed()).map(|_| 0),
        _ => None,
    };
    run_effects();
    match handled {
        Some(r) => r,
        None => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Window title identifies the instance (so a dev/test instance never talks to the real one).
fn instance_title() -> String {
    match std::env::var("PIP_DATA_DIR") {
        Ok(d) => {
            let h = d.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
            format!("Pip Companion [{h:016x}]")
        }
        Err(_) => "Pip Companion".into(),
    }
}

/// Quietly asks a running Pip to quit (used before replacing or deleting its executable).
pub fn send_command_to(_exe: &std::path::Path, cmd: &str) {
    unsafe {
        let hwnd = FindWindowW(wide(PET_CLASS).as_ptr(), wide(&instance_title()).as_ptr());
        if !hwnd.is_null() {
            let bytes = cmd.as_bytes();
            let cds = COPYDATASTRUCT { dwData: COPYDATA_MAGIC, cbData: bytes.len() as u32, lpData: bytes.as_ptr() as *mut _ };
            SendMessageW(hwnd, WM_COPYDATA, 0, &cds as *const _ as LPARAM);
        }
    }
}

/// `pip.exe --cmd "<command>"`: sends a command to the running instance, prints the reply.
fn send_command(cmd: &str) -> i32 {
    unsafe {
        let hwnd = FindWindowW(wide(PET_CLASS).as_ptr(), wide(&instance_title()).as_ptr());
        if hwnd.is_null() {
            eprintln!("Pip is not running");
            return 2;
        }
        let bytes = cmd.as_bytes();
        let cds = COPYDATASTRUCT { dwData: COPYDATA_MAGIC, cbData: bytes.len() as u32, lpData: bytes.as_ptr() as *mut _ };
        let r = SendMessageW(hwnd, WM_COPYDATA, 0, &cds as *const _ as LPARAM);
        if r == 0 {
            return 1;
        }
        if let Ok(reply) = std::fs::read_to_string(storage::Storage::open().dir().join("cmd-reply.txt")) {
            println!("{reply}");
        }
        0
    }
}

fn register_class(name: &str, proc_: WNDPROC, style: u32) {
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style,
            lpfnWndProc: proc_,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: wide(name).leak().as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        RegisterClassExW(&wc);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--cmd") {
        let cmd = args.get(i + 1).cloned().unwrap_or_default();
        // Attach to the parent console so replies are visible when run from a terminal.
        unsafe { windows_sys::Win32::System::Console::AttachConsole(u32::MAX) };
        let code = send_command(&cmd);
        std::process::exit(code);
    }

    if args.iter().any(|a| a == "--uninstall") {
        install::run_uninstall(args.iter().any(|a| a == "--silent"));
        return;
    }
    if args.iter().any(|a| a == "--install") {
        match install::install(true) {
            Ok(exe) => {
                let _ = std::process::Command::new(&exe).spawn();
            }
            Err(e) => {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                        std::ptr::null_mut(),
                        wide(&e).as_ptr(),
                        wide("Pip").as_ptr(),
                        windows_sys::Win32::UI::WindowsAndMessaging::MB_OK,
                    )
                };
            }
        }
        return;
    }
    // Started from a folder (download, USB stick, build directory): offer to install once.
    if install::offer_install_if_loose() {
        return; // the installed copy takes over
    }

    platform::set_dpi_awareness();
    let store = storage::Storage::open();
    crash::install(store.dir().join("pip.log"), platform::os_build());

    let mutex_name = format!("Local\\{}", instance_title().replace(['[', ']', ' '], "_"));
    if !platform::acquire_single_instance(&mutex_name) {
        // Already running: wake the existing pet instead of starting a second one.
        let _ = send_command("show");
        return;
    }

    let now = platform::now_unix();
    let (settings, existed) = store.load_settings();
    let state = store.load_state(now);

    register_class(PET_CLASS, Some(pet_proc), CS_DBLCLKS);
    register_class(SETTINGS_CLASS, Some(settings_proc), CS_HREDRAW | CS_VREDRAW);
    TASKBAR_CREATED.with(|t| t.set(unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) }));

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            wide(PET_CLASS).as_ptr(),
            wide(&instance_title()).as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        store.log("could not create the pet window");
        return;
    }
    // Let the command channel through UIPI when started elevated/unelevated in mixed setups.
    unsafe { ChangeWindowMessageFilterEx(hwnd, WM_COPYDATA, MSGFLT_ALLOW, std::ptr::null_mut()) };

    APP.with(|cell| *cell.borrow_mut() = Some(App::new(hwnd, store, settings, state, !existed)));
    with_app(|a| a.start());
    run_effects();

    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    with_app(|a| a.flush_save());
}
