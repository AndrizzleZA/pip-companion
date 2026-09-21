//! A GDI DIB section that tiny-skia draws into directly — the same memory Windows composites,
//! so rendering involves no intermediate buffers. Recreated only when the size changes.

use crate::render::rgba_to_bgra;
use tiny_skia::PixmapMut;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub struct Surface {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub w: i32,
    pub h: i32,
}

impl Surface {
    pub const fn empty() -> Surface {
        Surface { dc: std::ptr::null_mut(), bmp: std::ptr::null_mut(), old: std::ptr::null_mut(), bits: std::ptr::null_mut(), w: 0, h: 0 }
    }

    pub fn ensure(&mut self, w: i32, h: i32) -> bool {
        let (w, h) = (w.max(1), h.max(1));
        if w == self.w && h == self.h && !self.bits.is_null() {
            return true;
        }
        self.release();
        unsafe {
            let mut bi: BITMAPINFO = std::mem::zeroed();
            bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bi.bmiHeader.biWidth = w;
            bi.bmiHeader.biHeight = -h; // top-down rows, same order as tiny-skia
            bi.bmiHeader.biPlanes = 1;
            bi.bmiHeader.biBitCount = 32;
            bi.bmiHeader.biCompression = BI_RGB;
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if bmp.is_null() || bits.is_null() {
                DeleteDC(dc);
                return false;
            }
            self.old = SelectObject(dc, bmp as HGDIOBJ);
            self.dc = dc;
            self.bmp = bmp;
            self.bits = bits as *mut u8;
            self.w = w;
            self.h = h;
        }
        true
    }

    pub fn bytes(&mut self) -> &mut [u8] {
        if self.bits.is_null() {
            return &mut [];
        }
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h * 4) as usize) }
    }

    /// A cleared canvas over the DIB memory.
    pub fn canvas(&mut self) -> Option<PixmapMut<'_>> {
        let (w, h) = (self.w as u32, self.h as u32);
        let data = self.bytes();
        data.fill(0);
        PixmapMut::from_bytes(data, w, h)
    }

    /// Converts to GDI byte order and pushes the pixels to a per-pixel-alpha layered window.
    pub fn present_layered(&mut self, hwnd: HWND, x: i32, y: i32) {
        rgba_to_bgra(self.bytes());
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            let dst = POINT { x, y };
            let size = SIZE { cx: self.w, cy: self.h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            UpdateLayeredWindow(hwnd, screen, &dst, &size, self.dc, &src, 0, &blend, ULW_ALPHA);
            ReleaseDC(std::ptr::null_mut(), screen);
        }
    }

    /// Converts to GDI byte order and blits into a paint DC (opaque windows).
    pub fn blit(&mut self, hdc: HDC) {
        rgba_to_bgra(self.bytes());
        unsafe {
            BitBlt(hdc, 0, 0, self.w, self.h, self.dc, 0, 0, SRCCOPY);
        }
    }

    /// Current pixels as an RGBA premultiplied copy (dev snapshots, tray icons).
    #[cfg(feature = "devtools")]
    pub fn rgba_copy(&mut self) -> Vec<u8> {
        let mut v = self.bytes().to_vec();
        rgba_to_bgra(&mut v); // the swap is symmetric
        v
    }

    pub fn release(&mut self) {
        unsafe {
            if !self.dc.is_null() {
                SelectObject(self.dc, self.old);
                DeleteObject(self.bmp as HGDIOBJ);
                DeleteDC(self.dc);
            }
        }
        // Reset field by field: assigning a new Surface would run Drop on the old one (recursion).
        self.dc = std::ptr::null_mut();
        self.bmp = std::ptr::null_mut();
        self.old = std::ptr::null_mut();
        self.bits = std::ptr::null_mut();
        self.w = 0;
        self.h = 0;
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        self.release();
    }
}
