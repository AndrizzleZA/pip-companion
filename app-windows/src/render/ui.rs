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

/// How thick the lip under a button or panel is, in logical units (§73).
///
/// Everything you can press has one: a darker slab behind it, offset downwards, so the face sits
/// *on* something rather than being a coloured rectangle printed on the background. It is two
/// units, which is enough to read as a raised edge at 100% and still looks deliberate at 150%.
const LIP: f32 = 2.5;

/// A raised, pressable button: a dark lip, a face, and a highlight along the top of the face.
pub fn button(pm: &mut PixmapMut, r: R, label: &str, style: BtnStyle, pal: &Palette, s: f32, hover: bool) {
    let (bg, fg, a) = match style {
        BtnStyle::Primary => (pal.accent, pal.on_accent, 1.0),
        BtnStyle::Secondary => (pal.surface2, pal.text, 1.0),
        BtnStyle::Ghost => (pal.surface2, pal.muted, if hover { 1.0 } else { 0.0 }),
        BtnStyle::Danger => (if pal.dark { 0x8A3A48 } else { 0xE4607A }, 0xFFFFFF, 1.0),
    };
    let bg = if hover && style != BtnStyle::Ghost { mix(bg, 0xFFFFFF, 0.12) } else { bg };
    let rad = 10.0 * s;
    if a > 0.0 {
        // The lip first, then the face on top of it, shortened by the lip so the whole control
        // still occupies exactly `r` and the hit rectangle stays honest.
        let lip = LIP * s;
        fill_rrect(pm, r.x, r.y + lip, r.w, r.h - lip, rad, color(mix(bg, 0x000000, if pal.dark { 0.45 } else { 0.22 }), a));
        fill_rrect(pm, r.x, r.y, r.w, r.h - lip, rad, color(bg, a));
        // A hair of light along the top edge, which is what makes it read as rounded plastic
        // rather than as a flat card with a shadow.
        fill_rrect(pm, r.x + rad * 0.6, r.y + 1.5 * s, r.w - rad * 1.2, 1.5 * s, 1.0 * s, color(mix(bg, 0xFFFFFF, 0.5), a * 0.75));
    }
    let size = 12.5 * s;
    let face_h = if a > 0.0 { r.h - LIP * s } else { r.h };
    text::draw_centered(pm, &fonts().semibold, size, r.x + r.w / 2.0, baseline(r.y, face_h, size), fg, 1.0, label);
}

pub fn button_width(label: &str, s: f32) -> f32 {
    text::width(&fonts().semibold, 12.5 * s, label) + 30.0 * s
}

/// A raised panel for a group of rows: lip, face, and a light edge along the top (§73).
pub fn panel(pm: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, pal: &Palette, s: f32) {
    let rad = 16.0 * s;
    let lip = 3.0 * s;
    fill_rrect(pm, x, y + lip, w, h, rad, color(mix(pal.surface, 0x000000, if pal.dark { 0.5 } else { 0.1 }), 1.0));
    fill_rrect(pm, x, y, w, h, rad, color(pal.surface, 1.0));
    fill_rrect(pm, x + rad * 0.7, y + 2.0 * s, w - rad * 1.4, 1.5 * s, 1.0 * s, color(mix(pal.surface, 0xFFFFFF, if pal.dark { 0.22 } else { 0.9 }), 1.0));
}

/// The little coloured name plate that sits over the top edge of a panel, the way the title
/// boards do in the reference art. Returns its height, so the panel can start under it.
pub fn plate(pm: &mut PixmapMut, x: f32, y: f32, label: &str, pal: &Palette, s: f32) -> f32 {
    let size = 11.5 * s;
    let f = &fonts().semibold;
    let w = text::width(f, size, label) + 30.0 * s;
    let h = 26.0 * s;
    let rad = 9.0 * s;
    fill_rrect(pm, x, y + 2.5 * s, w, h, rad, color(mix(pal.accent, 0x000000, 0.4), 1.0));
    fill_rrect(pm, x, y, w, h, rad, color(pal.accent, 1.0));
    fill_rrect(pm, x + rad * 0.7, y + 2.0 * s, w - rad * 1.4, 1.5 * s, 1.0 * s, color(mix(pal.accent, 0xFFFFFF, 0.55), 0.8));
    text::draw_centered(pm, f, size, x + w / 2.0, baseline(y, h, size), pal.on_accent, 1.0, label);
    h
}

/// Earned stars, like the ones on a finished level. `filled` of `total`.
pub fn stars(pm: &mut PixmapMut, x: f32, cy: f32, filled: u32, total: u32, pal: &Palette, s: f32) -> f32 {
    let r = 9.0 * s;
    let step = r * 2.4;
    for i in 0..total {
        let cx = x + r + step * i as f32;
        let on = i < filled;
        let c = if on { 0xFFC94D } else if pal.dark { 0x3A404D } else { 0xD8DDE6 };
        star(pm, cx, cy, r, color(c, 1.0));
        if on {
            star(pm, cx, cy - 1.0 * s, r * 0.55, color(0xFFE9A8, 1.0));
        }
    }
    step * total as f32
}

fn star(pm: &mut PixmapMut, cx: f32, cy: f32, r: f32, c: tiny_skia::Color) {
    let mut pb = tiny_skia::PathBuilder::new();
    for i in 0..10 {
        let a = std::f32::consts::PI * 2.0 * (i as f32 / 10.0) - std::f32::consts::FRAC_PI_2;
        let rad = if i % 2 == 0 { r } else { r * 0.44 };
        let (px, py) = (cx + rad * a.cos(), cy + rad * a.sin());
        if i == 0 {
            pb.move_to(px, py);
        } else {
            pb.line_to(px, py);
        }
    }
    pb.close();
    super::fill_path(pm, pb.finish(), c, tiny_skia::Transform::identity());
}

pub fn toggle(pm: &mut PixmapMut, x: f32, y: f32, on: bool, pal: &Palette, s: f32) -> R {
    let (w, h) = (44.0 * s, 24.0 * s);
    let track = if on { pal.accent } else if pal.dark { 0x454B5A } else { 0xC3C9D4 };
    fill_rrect(pm, x, y + 2.0 * s, w, h, h / 2.0, color(mix(track, 0x000000, if pal.dark { 0.45 } else { 0.18 }), 1.0));
    fill_rrect(pm, x, y, w, h, h / 2.0, color(track, 1.0));
    let kx = if on { x + w - h / 2.0 } else { x + h / 2.0 };
    fill_circle(pm, kx, y + h / 2.0 + 1.5 * s, h / 2.0 - 3.5 * s, color(mix(0xFFFFFF, 0x000000, 0.3), 0.45));
    fill_circle(pm, kx, y + h / 2.0, h / 2.0 - 3.5 * s, color(0xFFFFFF, 1.0));
    R::new(x, y, w, h + 2.0 * s)
}

pub const TOGGLE_W: f32 = 44.0;

/// `[−]  value  [+]` with the value sitting on a filled gauge, the way an upgrade bar reads in
/// the reference art (§73). `fill` is 0..1, or `None` for a value with no range (a time of day).
///
/// It stays two buttons rather than becoming a draggable slider: the arrows are already reachable
/// by keyboard and already have hit rectangles, and a drag would be a new input mode in a window
/// that has never needed one.
#[allow(clippy::too_many_arguments)] // a drawing primitive: a struct here would be a struct nobody reads
pub fn stepper(pm: &mut PixmapMut, x: f32, y: f32, w: f32, value: &str, fill: Option<f32>, pal: &Palette, s: f32) -> (R, R) {
    let h = 32.0 * s;
    let rad = 10.0 * s;
    let trough = if pal.dark { 0x1A1D24 } else { 0xDFE3EA };
    fill_rrect(pm, x, y, w, h, rad, color(trough, 1.0));
    let bw = 32.0 * s;
    if let Some(t) = fill {
        // The bar lives between the two arrows, so a full gauge never looks like it has eaten
        // them. The empty groove is drawn whatever the value is: a bar with nothing in it still
        // says "this is a quantity", where a lone coloured square says nothing at all.
        let (gx, track_w) = (x + bw - 2.0 * s, w - bw * 2.0 + 4.0 * s);
        let (gy, gh) = (y + 6.0 * s, h - 12.0 * s);
        fill_rrect(pm, gx, gy, track_w, gh, gh / 2.0, color(mix(trough, 0x000000, if pal.dark { 0.35 } else { 0.08 }), 1.0));
        let gw = track_w * t.clamp(0.0, 1.0);
        if gw > gh * 0.6 {
            fill_rrect(pm, gx, gy, gw, gh, gh / 2.0, color(pal.accent, 1.0));
            fill_rrect(pm, gx + 3.0 * s, gy + 2.0 * s, (gw - 6.0 * s).max(0.0), 2.0 * s, 1.0 * s, color(mix(pal.accent, 0xFFFFFF, 0.5), 0.8));
        }
    }
    let minus = R::new(x, y, bw, h);
    let plus = R::new(x + w - bw, y, bw, h);
    let size = 17.0 * s;
    let f = &fonts().semibold;
    for (r, glyph) in [(minus, "−"), (plus, "+")] {
        fill_circle(pm, r.x + bw / 2.0, y + h / 2.0, 11.0 * s, color(if pal.dark { 0x39404F } else { 0xFFFFFF }, 1.0));
        text::draw_centered(pm, f, size, r.x + bw / 2.0, baseline(y, h, size) - 1.0 * s, pal.text, 1.0, glyph);
    }
    let vs = 13.0 * s;
    let on_bar = fill.is_some_and(|t| t > 0.45);
    let fg = if on_bar { pal.on_accent } else { pal.text };
    text::draw_centered(pm, f, vs, x + w / 2.0, baseline(y, h, vs), fg, 1.0, value);
    (minus, plus)
}

/// A row of independent on/off pills — several small switches that belong together, where a
/// label-and-toggle row each would be three rows saying almost the same thing (§73).
pub fn chips(pm: &mut PixmapMut, x: f32, y: f32, labels: &[&str], on: &[bool], pal: &Palette, s: f32) -> Vec<R> {
    let h = 30.0 * s;
    let size = 12.0 * s;
    let f = &fonts().semibold;
    let mut out = Vec::new();
    let mut cx = x;
    for (i, label) in labels.iter().enumerate() {
        let w = text::width(f, size, label) + 26.0 * s;
        let r = R::new(cx, y, w, h);
        let lit = on.get(i).copied().unwrap_or(false);
        let bg = if lit { pal.accent } else if pal.dark { 0x343B49 } else { 0xDFE3EA };
        fill_rrect(pm, r.x, r.y + 2.0 * s, r.w, r.h, h / 2.0, color(mix(bg, 0x000000, if pal.dark { 0.45 } else { 0.18 }), 1.0));
        fill_rrect(pm, r.x, r.y, r.w, r.h, h / 2.0, color(bg, 1.0));
        let fg = if lit { pal.on_accent } else { pal.muted };
        text::draw_centered(pm, f, size, r.x + r.w / 2.0, baseline(r.y, r.h, size), fg, 1.0, label);
        out.push(R::new(r.x, r.y, r.w, r.h + 2.0 * s));
        cx += w + 7.0 * s;
    }
    out
}

/// What `chips` will take up, so a caller can right-align it.
pub fn chips_width(labels: &[&str], s: f32) -> f32 {
    let f = &fonts().semibold;
    labels.iter().map(|l| text::width(f, 12.0 * s, l) + 33.0 * s).sum::<f32>() - 7.0 * s
}

pub fn segmented(pm: &mut PixmapMut, x: f32, y: f32, options: &[&str], selected: usize, pal: &Palette, s: f32) -> Vec<R> {
    let h = 30.0 * s;
    let size = 12.5 * s;
    let pad = 14.0 * s;
    let widths: Vec<f32> = options.iter().map(|o| text::width(&fonts().semibold, size, o) + pad * 2.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0 * s;
    let trough = if pal.dark { 0x1A1D24 } else { 0xDFE3EA };
    fill_rrect(pm, x, y, total, h, 10.0 * s, color(trough, 1.0));
    let mut out = Vec::new();
    let mut cx = x + 3.0 * s;
    for (i, (o, w)) in options.iter().zip(&widths).enumerate() {
        let r = R::new(cx, y + 3.0 * s, *w, h - 6.0 * s);
        if i == selected {
            // The chosen one is a raised, accent-coloured key sitting in the trough, so which
            // option is live reads from across the room rather than from a shade of grey.
            fill_rrect(pm, r.x, r.y + 1.5 * s, r.w, r.h, 7.0 * s, color(mix(pal.accent, 0x000000, 0.4), 1.0));
            fill_rrect(pm, r.x, r.y, r.w, r.h, 7.0 * s, color(pal.accent, 1.0));
            fill_rrect(pm, r.x + 5.0 * s, r.y + 1.5 * s, r.w - 10.0 * s, 1.5 * s, 1.0 * s, color(mix(pal.accent, 0xFFFFFF, 0.5), 0.75));
        }
        let fg = if i == selected { pal.on_accent } else { pal.muted };
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
        let (bg, fg) = if on { (pal.accent, pal.on_accent) } else { (if pal.dark { 0x343B49 } else { 0xDFE3EA }, pal.muted) };
        fill_circle(pm, cx + d / 2.0, y + d / 2.0 + 2.0 * s, d / 2.0, color(mix(bg, 0x000000, if pal.dark { 0.45 } else { 0.18 }), 1.0));
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
