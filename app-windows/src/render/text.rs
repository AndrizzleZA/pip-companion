//! Text via ab_glyph using the system's Segoe UI. Font files are memory-mapped read-only, so
//! they cost shared, reclaimable page cache rather than private memory, and nothing is bundled.

use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use std::sync::OnceLock;
use tiny_skia::PixmapMut;

pub struct Fonts {
    pub regular: FontRef<'static>,
    pub semibold: FontRef<'static>,
    /// Fallback for symbols Segoe UI lacks (✓ ★ ☆ …).
    pub symbol: Option<FontRef<'static>>,
}

static FONTS: OnceLock<Option<Fonts>> = OnceLock::new();

pub fn fonts() -> &'static Fonts {
    FONTS
        .get_or_init(|| {
            let dir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()) + "\\Fonts\\";
            let load = |names: &[&str]| {
                names.iter().find_map(|n| map_file(&(dir.clone() + n)).and_then(|b| FontRef::try_from_slice(b).ok()))
            };
            Some(Fonts {
                regular: load(&["segoeui.ttf", "arial.ttf"])?,
                semibold: load(&["seguisb.ttf", "segoeuib.ttf", "arialbd.ttf"])?,
                symbol: load(&["seguisym.ttf"]),
            })
        })
        .as_ref()
        .expect("system fonts (Segoe UI or Arial) must exist")
}

fn map_file(path: &str) -> Option<&'static [u8]> {
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Storage::FileSystem::*;
    use windows_sys::Win32::System::Memory::*;
    unsafe {
        let w = crate::platform::wide(path);
        let file = CreateFileW(
            w.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if file == INVALID_HANDLE_VALUE {
            return None;
        }
        let size = GetFileSize(file, std::ptr::null_mut()) as usize;
        let mapping = CreateFileMappingW(file, std::ptr::null(), PAGE_READONLY, 0, 0, std::ptr::null());
        CloseHandle(file);
        if mapping.is_null() {
            return None;
        }
        let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
        CloseHandle(mapping); // the view keeps the mapping alive
        if view.Value.is_null() || size == 0 {
            return None;
        }
        // Mapped for the life of the process (fonts are shared and immutable).
        Some(std::slice::from_raw_parts(view.Value as *const u8, size))
    }
}

pub fn ascent(font: &FontRef, size: f32) -> f32 {
    font.as_scaled(PxScale::from(size)).ascent()
}

pub fn width(font: &FontRef, size: f32, text: &str) -> f32 {
    let sf = font.as_scaled(PxScale::from(size));
    let mut w = 0.0;
    let mut prev = None;
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        if id.0 == 0 {
            if let Some(sym) = &fonts().symbol {
                let ss = sym.as_scaled(PxScale::from(size));
                w += ss.h_advance(ss.glyph_id(ch));
                prev = None;
                continue;
            }
        }
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Draws `text` with its baseline at `y`. Returns the advance width.
pub fn draw(pm: &mut PixmapMut, font: &FontRef, size: f32, x: f32, y: f32, rgb: u32, alpha: f32, text: &str) -> f32 {
    let sf = font.as_scaled(PxScale::from(size));
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let (r, g, b) = (((rgb >> 16) & 0xff) as f32, ((rgb >> 8) & 0xff) as f32, (rgb & 0xff) as f32);
    let mut caret = x;
    let mut prev = None;
    for ch in text.chars() {
        let mut id = sf.glyph_id(ch);
        let mut face: &FontRef = font;
        if id.0 == 0 {
            if let Some(sym) = &fonts().symbol {
                face = sym;
                id = sym.glyph_id(ch);
                prev = None;
            }
        }
        let fsf = face.as_scaled(PxScale::from(size));
        if let Some(p) = prev {
            caret += sf.kern(p, id);
        }
        let glyph = id.with_scale_and_position(fsf.scale(), point(caret, y));
        caret += fsf.h_advance(id);
        prev = if std::ptr::eq(face, font) { Some(id) } else { None };
        let Some(outline) = face.outline_glyph(glyph) else { continue };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, cov| {
            let px = bounds.min.x as i32 + gx as i32;
            let py = bounds.min.y as i32 + gy as i32;
            if px < 0 || py < 0 || px >= w || py >= h {
                return;
            }
            let sa = (cov * alpha).clamp(0.0, 1.0);
            if sa <= 0.0 {
                return;
            }
            let i = ((py * w + px) * 4) as usize;
            let inv = 1.0 - sa;
            data[i] = (r * sa + data[i] as f32 * inv) as u8;
            data[i + 1] = (g * sa + data[i + 1] as f32 * inv) as u8;
            data[i + 2] = (b * sa + data[i + 2] as f32 * inv) as u8;
            data[i + 3] = (255.0 * sa + data[i + 3] as f32 * inv) as u8;
        });
    }
    caret - x
}

pub fn draw_centered(pm: &mut PixmapMut, font: &FontRef, size: f32, cx: f32, y: f32, rgb: u32, alpha: f32, text: &str) {
    let w = width(font, size, text);
    draw(pm, font, size, cx - w / 2.0, y, rgb, alpha, text);
}

/// Greedy word wrap. Very long words are broken by character.
pub fn wrap(font: &FontRef, size: f32, text: &str, max_w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let candidate = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if width(font, size, &candidate) <= max_w {
            cur = candidate;
            continue;
        }
        if !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
        }
        if width(font, size, word) <= max_w {
            cur = word.to_string();
        } else {
            for ch in word.chars() {
                let mut t = cur.clone();
                t.push(ch);
                if width(font, size, &t) > max_w && !cur.is_empty() {
                    lines.push(std::mem::take(&mut cur));
                }
                cur.push(ch);
            }
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Truncates with an ellipsis to fit `max_w`.
pub fn ellipsize(font: &FontRef, size: f32, text: &str, max_w: f32) -> String {
    if width(font, size, text) <= max_w {
        return text.to_string();
    }
    let mut s: String = text.to_string();
    while !s.is_empty() && width(font, size, &(s.clone() + "…")) > max_w {
        s.pop();
    }
    s + "…"
}
