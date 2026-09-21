//! Software 2D rendering helpers on top of tiny-skia. All drawing happens into caller-owned
//! pixel buffers (usually the DIB section Windows composites), so there are no extra copies.

pub mod creature;
pub mod text;
pub mod ui;

use pip_core::settings::{PetColor, Theme as ThemePref};
use tiny_skia::*;

pub fn color(rgb: u32, alpha: f32) -> Color {
    Color::from_rgba8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let x = ((a >> s) & 0xff) as f32;
        let y = ((b >> s) & 0xff) as f32;
        ((x + (y - x) * t).round() as u32).min(255) << s
    };
    ch(16) | ch(8) | ch(0)
}

pub fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

pub fn rrect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = 0.5523 * r; // cubic approximation of a quarter circle
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

pub fn ellipse_path(cx: f32, cy: f32, rx: f32, ry: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.push_oval(Rect::from_xywh(cx - rx, cy - ry, rx * 2.0, ry * 2.0)?);
    pb.finish()
}

pub fn fill_path(pm: &mut PixmapMut, path: Option<Path>, c: Color, ts: Transform) {
    if let Some(p) = path {
        pm.fill_path(&p, &paint(c), FillRule::Winding, ts, None);
    }
}

pub fn fill_rrect(pm: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, r: f32, c: Color) {
    fill_path(pm, rrect_path(x, y, w, h, r), c, Transform::identity());
}

pub fn stroke_rrect(pm: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, r: f32, c: Color, width: f32) {
    if let Some(p) = rrect_path(x, y, w, h, r) {
        let s = Stroke { width, ..Stroke::default() };
        pm.stroke_path(&p, &paint(c), &s, Transform::identity(), None);
    }
}

pub fn fill_circle(pm: &mut PixmapMut, cx: f32, cy: f32, r: f32, c: Color) {
    fill_path(pm, PathBuilder::from_circle(cx, cy, r), c, Transform::identity());
}

pub fn stroke_path(pm: &mut PixmapMut, path: Option<Path>, c: Color, width: f32, ts: Transform) {
    if let Some(p) = path {
        let s = Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
        pm.stroke_path(&p, &paint(c), &s, ts, None);
    }
}

/// Cheap soft shadow: a few expanding translucent rounded rects (no blur pass).
pub fn soft_shadow(pm: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, r: f32, s: f32, alpha: f32) {
    for i in 1..=4 {
        let g = i as f32 * 1.6 * s;
        fill_rrect(pm, x - g, y - g + 2.0 * s, w + 2.0 * g, h + 2.0 * g, r + g, color(0, alpha / (i as f32 + 1.0)));
    }
}

/// tiny-skia writes premultiplied RGBA; GDI wants premultiplied BGRA.
pub fn rgba_to_bgra(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}

/// Resolved colours for one render.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub window: u32,
    pub surface: u32,
    pub surface2: u32,
    pub text: u32,
    pub muted: u32,
    pub border: u32,
    pub accent: u32,
    pub on_accent: u32,
    pub body: u32,
}

impl Palette {
    pub fn new(pref: ThemePref, pet: PetColor) -> Palette {
        let dark = match pref {
            ThemePref::Dark => true,
            ThemePref::Light => false,
            ThemePref::System => crate::platform::system_dark_mode(),
        };
        let accent = pet.accent();
        if dark {
            Palette {
                dark,
                window: 0x1C1F26,
                surface: 0x262A33,
                surface2: 0x323744,
                text: 0xF2F4F8,
                muted: 0x9AA3B2,
                border: 0x3A404D,
                accent: mix(accent, 0xFFFFFF, 0.18),
                on_accent: 0xFFFFFF,
                body: pet.rgb(),
            }
        } else {
            Palette {
                dark,
                window: 0xF6F7F9,
                surface: 0xFFFFFF,
                surface2: 0xEEF0F4,
                text: 0x1E2330,
                muted: 0x6B7385,
                border: 0xE1E4EA,
                accent,
                on_accent: 0xFFFFFF,
                body: pet.rgb(),
            }
        }
    }
}
