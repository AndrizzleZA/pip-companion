//! Minimal immediate-mode widget painting shared by the speech bubble and the settings window.
//! Widgets only draw; callers record hit rectangles and handle input themselves.

use super::text::{self, fonts};
use super::{color, fill_circle, fill_rrect, mix, stroke_rrect, Palette};
use tiny_skia::PixmapMut;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> R {
        R { x, y, w, h }
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BtnStyle {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// Vertically centred text baseline inside a box of height `h` starting at `y`.
pub fn baseline(y: f32, h: f32, size: f32) -> f32 {
    y + h / 2.0 + size * 0.36
}

pub fn button(pm: &mut PixmapMut, r: R, label: &str, style: BtnStyle, pal: &Palette, s: f32, hover: bool) {
    let (bg, fg, a) = match style {
        BtnStyle::Primary => (pal.accent, pal.on_accent, 1.0),
        BtnStyle::Secondary => (pal.surface2, pal.text, 1.0),
        BtnStyle::Ghost => (pal.surface2, pal.muted, if hover { 1.0 } else { 0.0 }),
        BtnStyle::Danger => (if pal.dark { 0x4A2A30 } else { 0xFDECEE }, 0xD6455D, 1.0),
    };
    let bg = if hover && style != BtnStyle::Ghost { mix(bg, if pal.dark { 0xFFFFFF } else { 0x000000 }, 0.08) } else { bg };
    if a > 0.0 {
        fill_rrect(pm, r.x, r.y, r.w, r.h, r.h / 2.0, color(bg, a));
    }
    let size = 12.5 * s;
    text::draw_centered(pm, &fonts().semibold, size, r.x + r.w / 2.0, baseline(r.y, r.h, size), fg, 1.0, label);
}

pub fn button_width(label: &str, s: f32) -> f32 {
    text::width(&fonts().semibold, 12.5 * s, label) + 26.0 * s
}

pub fn toggle(pm: &mut PixmapMut, x: f32, y: f32, on: bool, pal: &Palette, s: f32) -> R {
    let (w, h) = (40.0 * s, 22.0 * s);
    let track = if on { pal.accent } else if pal.dark { 0x4A5060 } else { 0xC9CED8 };
    fill_rrect(pm, x, y, w, h, h / 2.0, color(track, 1.0));
    let kx = if on { x + w - h / 2.0 } else { x + h / 2.0 };
    fill_circle(pm, kx, y + h / 2.0, h / 2.0 - 3.0 * s, color(0xFFFFFF, 1.0));
    R::new(x, y, w, h)
}

pub const TOGGLE_W: f32 = 40.0;

/// `[−] value [+]`. Returns (minus, plus) hit rects.
pub fn stepper(pm: &mut PixmapMut, x: f32, y: f32, w: f32, value: &str, pal: &Palette, s: f32) -> (R, R) {
    let h = 30.0 * s;
    fill_rrect(pm, x, y, w, h, 9.0 * s, color(pal.surface2, 1.0));
    let bw = 30.0 * s;
    let minus = R::new(x, y, bw, h);
    let plus = R::new(x + w - bw, y, bw, h);
    let size = 16.0 * s;
    let f = &fonts().semibold;
    text::draw_centered(pm, f, size, minus.x + bw / 2.0, baseline(y, h, size) - 1.0 * s, pal.muted, 1.0, "−");
    text::draw_centered(pm, f, size, plus.x + bw / 2.0, baseline(y, h, size) - 1.0 * s, pal.muted, 1.0, "+");
    let vs = 13.0 * s;
    text::draw_centered(pm, f, vs, x + w / 2.0, baseline(y, h, vs), pal.text, 1.0, value);
    (minus, plus)
}

pub fn segmented(pm: &mut PixmapMut, x: f32, y: f32, options: &[&str], selected: usize, pal: &Palette, s: f32) -> Vec<R> {
    let h = 30.0 * s;
    let size = 12.5 * s;
    let pad = 14.0 * s;
    let widths: Vec<f32> = options.iter().map(|o| text::width(&fonts().semibold, size, o) + pad * 2.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0 * s;
    fill_rrect(pm, x, y, total, h, 9.0 * s, color(pal.surface2, 1.0));
    let mut out = Vec::new();
    let mut cx = x + 3.0 * s;
    for (i, (o, w)) in options.iter().zip(&widths).enumerate() {
        let r = R::new(cx, y + 3.0 * s, *w, h - 6.0 * s);
        if i == selected {
            fill_rrect(pm, r.x, r.y, r.w, r.h, 7.0 * s, color(pal.surface, 1.0));
            if !pal.dark {
                stroke_rrect(pm, r.x, r.y, r.w, r.h, 7.0 * s, color(0x000000, 0.06), 1.0);
            }
        }
        let fg = if i == selected { pal.text } else { pal.muted };
        text::draw_centered(pm, &fonts().semibold, size, r.x + r.w / 2.0, baseline(r.y, r.h, size), fg, 1.0, o);
        out.push(r);
        cx += w;
    }
    out
}

pub fn segmented_width(options: &[&str], s: f32) -> f32 {
    options.iter().map(|o| text::width(&fonts().semibold, 12.5 * s, o) + 28.0 * s).sum::<f32>() + 6.0 * s
}

/// Seven weekday chips M T W T F S S. Returns hit rects (Monday first).
pub fn day_chips(pm: &mut PixmapMut, x: f32, y: f32, mask: u8, pal: &Palette, s: f32) -> Vec<R> {
    const L: [&str; 7] = ["M", "T", "W", "T", "F", "S", "S"];
    let d = 26.0 * s;
    let gap = 5.0 * s;
    let mut out = Vec::new();
    for (i, l) in L.iter().enumerate() {
        let cx = x + i as f32 * (d + gap);
        let on = mask & (1 << i) != 0;
        let (bg, fg) = if on { (pal.accent, pal.on_accent) } else { (pal.surface2, pal.muted) };
        fill_circle(pm, cx + d / 2.0, y + d / 2.0, d / 2.0, color(bg, 1.0));
        let size = 11.5 * s;
        text::draw_centered(pm, &fonts().semibold, size, cx + d / 2.0, baseline(y, d, size), fg, 1.0, l);
        out.push(R::new(cx, y, d, d));
    }
    out
}

pub const DAY_CHIPS_W: f32 = 7.0 * 26.0 + 6.0 * 5.0;

pub fn text_field(pm: &mut PixmapMut, r: R, value: &str, focused: bool, pal: &Palette, s: f32, placeholder: &str) {
    fill_rrect(pm, r.x, r.y, r.w, r.h, 9.0 * s, color(pal.surface2, 1.0));
    if focused {
        stroke_rrect(pm, r.x + 0.5, r.y + 0.5, r.w - 1.0, r.h - 1.0, 9.0 * s, color(pal.accent, 1.0), 1.6 * s);
    }
    let size = 13.0 * s;
    let f = &fonts().regular;
    let pad = 10.0 * s;
    let shown = text::ellipsize(f, size, value, r.w - pad * 2.0 - 4.0 * s);
    // The hint stays while the field is focused and still empty, with the cursor in front of it.
    // It used to vanish on focus, which hid it at the one moment it was needed: a reminder is
    // created already focused, so the person was looking at an empty box and a cursor (§52).
    if value.is_empty() {
        text::draw(pm, f, size, r.x + pad, baseline(r.y, r.h, size), pal.muted, 0.8, placeholder);
        if focused {
            fill_rrect(pm, r.x + pad, r.y + 7.0 * s, 1.5 * s, r.h - 14.0 * s, 0.7 * s, color(pal.accent, 1.0));
        }
    } else {
        let w = text::draw(pm, f, size, r.x + pad, baseline(r.y, r.h, size), pal.text, 1.0, &shown);
        if focused {
            let cx = r.x + pad + w + 1.0 * s;
            fill_rrect(pm, cx, r.y + 7.0 * s, 1.5 * s, r.h - 14.0 * s, 0.7 * s, color(pal.accent, 1.0));
        }
    }
}

pub fn swatch(pm: &mut PixmapMut, cx: f32, cy: f32, rgb: u32, selected: bool, pal: &Palette, s: f32) -> R {
    let r = 11.0 * s;
    if selected {
        fill_circle(pm, cx, cy, r + 4.0 * s, color(pal.accent, 1.0));
        fill_circle(pm, cx, cy, r + 2.0 * s, color(pal.surface, 1.0));
    }
    fill_circle(pm, cx, cy, r, color(rgb, 1.0));
    R::new(cx - r - 4.0 * s, cy - r - 4.0 * s, (r + 4.0 * s) * 2.0, (r + 4.0 * s) * 2.0)
}
