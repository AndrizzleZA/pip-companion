//! Pip's creature, and the handful of tiny-skia helpers it is drawn with.
//!
//! Nothing in this crate knows what it is drawing into. Everything renders to a caller-owned
//! pixel buffer - the DIB section Windows composites, or an Android Bitmap - so the same creature
//! is the same creature on both, rather than two drawings that have to be kept looking alike.

pub mod creature;

/// Re-exported so a shell draws with exactly the tiny-skia this crate was built against, rather
/// than its own copy that happens to have the same version number.
pub use tiny_skia;

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

