//! Notification-area icon. The icon is the creature itself, rendered at the exact system size.

use crate::render::creature::{self, Expr, Pose};
use crate::render::{color, fill_circle};
use tiny_skia::{PixmapMut, Transform};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const TRAY_ID: u32 = 1;

pub struct Tray {
    hwnd: HWND,
    callback: u32,
    icon: HICON,
    icon_key: (u32, bool),
    pub added: bool,
    tip: String,
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

impl Tray {
    pub fn new(hwnd: HWND, callback: u32) -> Tray {
        Tray { hwnd, callback, icon: std::ptr::null_mut(), icon_key: (0, false), added: false, tip: String::new() }
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = TRAY_ID;
        nid
    }

    /// Adds (or re-adds after Explorer restarts) the icon.
    pub fn add(&mut self, body: u32, attention: bool, tip: &str) {
        self.ensure_icon(body, attention);
        let mut nid = self.data();
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        nid.uCallbackMessage = self.callback;
        nid.hIcon = self.icon;
        copy_wide(&mut nid.szTip, tip);
        self.tip = tip.to_string();
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &nid);
            self.added = Shell_NotifyIconW(NIM_ADD, &nid) != 0;
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            Shell_NotifyIconW(NIM_SETVERSION, &nid);
        }
    }

    /// Updates icon/tooltip only when they actually changed.
    pub fn update(&mut self, body: u32, attention: bool, tip: &str) {
        if !self.added {
            return;
        }
        let icon_changed = self.ensure_icon(body, attention);
        if !icon_changed && tip == self.tip {
            return;
        }
        let mut nid = self.data();
        nid.uFlags = NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        nid.hIcon = self.icon;
        copy_wide(&mut nid.szTip, tip);
        self.tip = tip.to_string();
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
    }

    /// A native Windows notification, used only when the pet itself is hidden.
    pub fn balloon(&mut self, title: &str, body: &str) {
        if !self.added {
            return;
        }
        let mut nid = self.data();
        nid.uFlags = NIF_INFO;
        copy_wide(&mut nid.szInfoTitle, title);
        copy_wide(&mut nid.szInfo, body);
        // We play our own soft chime (or nothing), never the system toast sound.
        nid.dwInfoFlags = NIIF_USER | NIIF_LARGE_ICON | NIIF_NOSOUND;
        nid.hBalloonIcon = self.icon;
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
    }

    pub fn remove(&mut self) {
        let nid = self.data();
        unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
        self.added = false;
    }

    fn ensure_icon(&mut self, body: u32, attention: bool) -> bool {
        if !self.icon.is_null() && self.icon_key == (body, attention) {
            return false;
        }
        let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(self.hwnd) }.max(96);
        let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) }.max(16);
        let new_icon = make_icon(body, attention, size);
        if !new_icon.is_null() {
            if !self.icon.is_null() {
                unsafe { DestroyIcon(self.icon) };
            }
            self.icon = new_icon;
            self.icon_key = (body, attention);
        }
        true
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        if !self.icon.is_null() {
            unsafe { DestroyIcon(self.icon) };
        }
    }
}

/// Renders the creature's face-on silhouette into an HICON.
pub fn make_icon(body: u32, attention: bool, size: i32) -> HICON {
    let n = size as u32;
    let mut buf = vec![0u8; (n * n * 4) as usize];
    {
        let Some(mut pm) = PixmapMut::from_bytes(&mut buf, n, n) else { return std::ptr::null_mut() };
        // Crop to the body (design x 12..88, y 12..96) so the icon is mostly creature.
        let s = size as f32 / 82.0;
        let base = Transform::from_scale(s, s).pre_translate(-9.0, -13.0);
        let mut pose = Pose::base(Expr::Content);
        pose.blush = 0.6;
        creature::draw(&mut pm, &pose, body, base);
        if attention {
            let r = size as f32 * 0.2;
            fill_circle(&mut pm, size as f32 - r - 0.5, r + 0.5, r + 1.0, color(0xFFFFFF, 1.0));
            fill_circle(&mut pm, size as f32 - r - 0.5, r + 0.5, r, color(0xFF5A6E, 1.0));
        }
    }
    // Icons want straight (non-premultiplied) BGRA.
    for px in buf.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in 0..3 {
                px[c] = ((px[c] as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
        px.swap(0, 2);
    }
    unsafe {
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = size;
        bi.bmiHeader.biHeight = -size;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color_bmp = CreateDIBSection(std::ptr::null_mut(), &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if color_bmp.is_null() {
            return std::ptr::null_mut();
        }
        std::ptr::copy_nonoverlapping(buf.as_ptr(), bits as *mut u8, buf.len());
        let mask = CreateBitmap(size, size, 1, 1, std::ptr::null());
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color_bmp };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color_bmp as HGDIOBJ);
        DeleteObject(mask as HGDIOBJ);
        icon
    }
}

