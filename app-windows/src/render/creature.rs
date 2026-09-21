//! The creature, drawn procedurally from a handful of pose parameters.
//!
//! Drawing is vector-based in a 100×100 design space and scaled at render time, so it is crisp on
//! every DPI and recolouring costs nothing. Animations are pure functions of (kind, t) that tweak a
//! `Pose`; the window only renders frames while an animation is actually playing.

use super::{color, ellipse_path, fill_path, mix, stroke_path};
use std::f32::consts::PI;
use tiny_skia::*;

/// Where the feet touch the ground in design space.
pub const GROUND_Y: f32 = 94.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eyes {
    Open,
    Wide,
    Happy,
    Closed,
    Sleepy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouth {
    Smile,
    Grin,
    O,
    Wobble,
    Tiny,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acc {
    None,
    Zzz,
    Drop,
    Heart,
    Sparkles,
    Alert,
    /// A water drop travelling to the mouth; progress in `acc_t`.
    Sip,
}

/// Resting expression derived from mood/status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expr {
    Content,
    Happy,
    Proud,
    Thirsty,
    Asleep,
    Carried,
}

#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub dy: f32,
    pub sx: f32,
    pub sy: f32,
    pub tilt: f32,
    pub eyes: Eyes,
    pub blink: f32,
    pub look_x: f32,
    pub look_y: f32,
    pub mouth: Mouth,
    pub blush: f32,
    pub leaf: f32,
    pub arm_l: f32,
    pub arm_r: f32,
    pub feet_drop: f32,
    pub acc: Acc,
    pub acc_t: f32,
}

impl Pose {
    pub fn base(expr: Expr) -> Pose {
        let mut p = Pose {
            dy: 0.0,
            sx: 1.0,
            sy: 1.0,
            tilt: 0.0,
            eyes: Eyes::Open,
            blink: 0.0,
            look_x: 0.0,
            look_y: 0.0,
            mouth: Mouth::Smile,
            blush: 0.4,
            leaf: 0.0,
            arm_l: 0.0,
            arm_r: 0.0,
            feet_drop: 0.0,
            acc: Acc::None,
            acc_t: 0.0,
        };
        match expr {
            Expr::Content => {}
            Expr::Happy => {
                p.mouth = Mouth::Grin;
                p.blush = 0.55;
            }
            Expr::Proud => {
                p.mouth = Mouth::Grin;
                p.blush = 0.65;
                p.leaf = 6.0;
            }
            Expr::Thirsty => {
                p.mouth = Mouth::Wobble;
                p.blush = 0.2;
                p.leaf = -22.0; // the sprout droops — a plant-like "I need water"
                p.look_y = 0.3;
            }
            Expr::Asleep => {
                p.eyes = Eyes::Closed;
                p.mouth = Mouth::Tiny;
                p.acc = Acc::Zzz;
                p.sy = 0.96;
                p.sx = 1.03;
                p.leaf = -10.0;
                p.blush = 0.5;
            }
            Expr::Carried => {
                p.eyes = Eyes::Wide;
                p.mouth = Mouth::O;
                p.arm_l = 0.7;
                p.arm_r = 0.7;
                p.feet_drop = 3.0;
                p.dy = -2.0;
            }
        }
        p
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimKind {
    Blink,
    Hop,
    Wave,
    Celebrate,
    Drink,
    LookAround,
    Stretch,
    Wiggle,
    WakeUp,
    PopIn,
    Nod,
}

impl AnimKind {
    pub fn duration_ms(self) -> u64 {
        match self {
            AnimKind::Blink => 150,
            AnimKind::Hop => 520,
            AnimKind::Wave => 1100,
            AnimKind::Celebrate => 1500,
            AnimKind::Drink => 1500,
            AnimKind::LookAround => 1900,
            AnimKind::Stretch => 1300,
            AnimKind::Wiggle => 700,
            AnimKind::WakeUp => 1100,
            AnimKind::PopIn => 450,
            AnimKind::Nod => 600,
        }
    }
}

fn smooth(a: f32, b: f32, t: f32) -> f32 {
    let x = ((t - a) / (b - a)).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Applies animation `kind` at normalised time `t` (0..=1) on top of a resting pose.
pub fn animate(kind: AnimKind, t: f32, p: &mut Pose) {
    let t = t.clamp(0.0, 1.0);
    match kind {
        AnimKind::Blink => {
            if matches!(p.eyes, Eyes::Open | Eyes::Wide) {
                p.blink = (t * PI).sin();
            }
        }
        AnimKind::Hop => {
            let h = (t * PI).sin();
            let edge = (1.0 - t / 0.16).max(0.0) + (1.0 - (1.0 - t) / 0.16).max(0.0);
            p.dy -= 13.0 * h;
            p.sy *= 1.0 + 0.07 * h - 0.10 * edge;
            p.sx *= 1.0 - 0.04 * h + 0.08 * edge;
            if h > 0.35 {
                p.eyes = Eyes::Happy;
            }
            p.mouth = Mouth::Grin;
            p.arm_l = p.arm_l.max(0.5 * h);
            p.arm_r = p.arm_r.max(0.5 * h);
        }
        AnimKind::Wave => {
            let ramp = smooth(0.0, 0.15, t) * (1.0 - smooth(0.85, 1.0, t));
            p.arm_r = ramp * (0.75 + 0.25 * (t * 6.0 * PI).sin());
            p.leaf += 10.0 * (t * 6.0 * PI).sin() * ramp;
            p.dy -= 3.0 * (t * 2.0 * PI).sin().abs();
            p.mouth = Mouth::Grin;
            p.blush = p.blush.max(0.6);
            if p.eyes != Eyes::Happy {
                p.eyes = Eyes::Open;
            }
        }
        AnimKind::Celebrate => {
            let h = (t * 2.0 * PI).sin().abs();
            p.dy -= 14.0 * h;
            p.sy *= 1.0 + 0.05 * h;
            p.arm_l = 0.4 + 0.6 * h;
            p.arm_r = 0.4 + 0.6 * h;
            p.eyes = Eyes::Happy;
            p.mouth = Mouth::Grin;
            p.blush = 0.75;
            p.acc = Acc::Sparkles;
            p.acc_t = t;
            p.leaf += 12.0 * (t * 4.0 * PI).sin();
        }
        AnimKind::Drink => {
            if t < 0.55 {
                p.acc = Acc::Sip;
                p.acc_t = t / 0.55;
                p.mouth = Mouth::O;
                p.eyes = Eyes::Wide;
                p.look_x = 0.8 - 1.2 * p.acc_t;
                p.look_y = -0.8 + 1.4 * p.acc_t;
            } else if t < 0.75 {
                let g = ((t - 0.55) / 0.2 * PI).sin();
                p.sy *= 1.0 - 0.07 * g;
                p.sx *= 1.0 + 0.05 * g;
                p.mouth = Mouth::Tiny;
                p.eyes = Eyes::Closed;
                p.acc = Acc::None;
            } else {
                let k = (t - 0.75) / 0.25;
                p.eyes = Eyes::Happy;
                p.mouth = Mouth::Grin;
                p.acc = Acc::Heart;
                p.acc_t = k;
                p.dy -= 5.0 * (k * PI).sin();
                p.leaf = p.leaf.max(0.0) + 8.0 * (k * PI).sin();
                p.blush = 0.7;
            }
        }
        AnimKind::LookAround => {
            let x = -smooth(0.08, 0.25, t) + 2.0 * smooth(0.5, 0.65, t) - smooth(0.85, 1.0, t);
            p.look_x = x;
            p.tilt += 3.0 * x;
        }
        AnimKind::Stretch => {
            let s = (t * PI).sin();
            p.sy *= 1.0 + 0.13 * s;
            p.sx *= 1.0 - 0.07 * s;
            p.arm_l = s;
            p.arm_r = s;
            if s > 0.4 {
                p.eyes = Eyes::Closed;
                p.mouth = Mouth::O;
            }
        }
        AnimKind::Wiggle => {
            let w = (t * 4.0 * PI).sin() * (1.0 - t);
            p.tilt += 7.0 * w;
            p.leaf += 14.0 * w;
            p.mouth = Mouth::Grin;
        }
        AnimKind::WakeUp => {
            if t < 0.35 {
                p.eyes = Eyes::Sleepy;
                p.mouth = Mouth::Tiny;
            } else {
                let s = ((t - 0.35) / 0.65 * PI).sin();
                p.sy *= 1.0 + 0.11 * s;
                p.sx *= 1.0 - 0.06 * s;
                p.arm_l = s;
                p.arm_r = s;
                p.eyes = if s > 0.5 { Eyes::Closed } else { Eyes::Open };
                p.mouth = if s > 0.5 { Mouth::O } else { Mouth::Smile };
            }
            p.acc = Acc::None;
            p.leaf = -10.0 * (1.0 - t);
        }
        AnimKind::PopIn => {
            // easeOutBack
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            let e = 1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2);
            let s = 0.4 + 0.6 * e;
            p.sx *= s;
            p.sy *= s;
        }
        AnimKind::Nod => {
            p.dy += 2.5 * (t * 4.0 * PI).sin().max(0.0);
            p.eyes = Eyes::Happy;
            p.mouth = Mouth::Smile;
        }
    }
}

const INK: u32 = 0x2A2A3A;
const LEAF: u32 = 0x4FA86A;
const WATER: u32 = 0x4DB3F2;

/// Draws the creature. `base` maps design space (0..100) into the target pixmap.
pub fn draw(pm: &mut PixmapMut, pose: &Pose, body: u32, base: Transform) {
    let body_light = mix(body, 0xFFFFFF, 0.38);
    let body_dark = mix(body, 0x000000, 0.18);

    // Ground shadow: stays on the ground, shrinks while airborne.
    let lift = (-pose.dy).max(0.0);
    let sh = (1.0 - lift / 55.0).clamp(0.4, 1.0);
    fill_path(pm, ellipse_path(50.0, GROUND_Y + 0.5, 25.0 * sh * pose.sx, 3.6 * sh), color(0, 0.16 * sh), base);

    // Body transform: squash/stretch and tilt around the feet.
    let t = base
        .pre_translate(50.0, GROUND_Y + pose.dy)
        .pre_concat(Transform::from_rotate(pose.tilt))
        .pre_scale(pose.sx, pose.sy)
        .pre_translate(-50.0, -GROUND_Y);

    // Feet
    for fx in [38.0, 62.0] {
        fill_path(pm, ellipse_path(fx, 91.0 + pose.feet_drop, 8.0, 4.6), color(body_dark, 1.0), t);
    }

    // Arms (behind the body so they peek out as little nubs)
    draw_arm(pm, t, 21.0, 63.0, -1.0, pose.arm_l, body_dark);
    draw_arm(pm, t, 79.0, 63.0, 1.0, pose.arm_r, body_dark);

    // Body: a soft mochi/egg shape with a vertical gradient.
    let mut pb = PathBuilder::new();
    pb.move_to(50.0, 27.0);
    pb.cubic_to(72.0, 27.0, 85.0, 47.0, 85.0, 66.0);
    pb.cubic_to(85.0, 85.0, 71.0, 93.0, 50.0, 93.0);
    pb.cubic_to(29.0, 93.0, 15.0, 85.0, 15.0, 66.0);
    pb.cubic_to(15.0, 47.0, 28.0, 27.0, 50.0, 27.0);
    pb.close();
    if let Some(path) = pb.finish() {
        let shader = LinearGradient::new(
            Point::from_xy(50.0, 27.0),
            Point::from_xy(50.0, 93.0),
            vec![GradientStop::new(0.0, color(body_light, 1.0)), GradientStop::new(1.0, color(body, 1.0))],
            SpreadMode::Pad,
            Transform::identity(),
        );
        let mut paint = Paint::default();
        paint.anti_alias = true;
        paint.shader = shader.unwrap_or(Shader::SolidColor(color(body, 1.0)));
        pm.fill_path(&path, &paint, FillRule::Winding, t, None);
        // Very subtle rim so light colours read on white desktops.
        stroke_path(pm, Some(path), color(body_dark, 0.35), 0.8, t);
    }

    // Belly and shine
    fill_path(pm, ellipse_path(50.0, 77.0, 19.0, 12.5), color(mix(body, 0xFFFFFF, 0.5), 0.5), t);
    fill_path(pm, ellipse_path(0.0, 0.0, 7.5, 4.0), color(0xFFFFFF, 0.6), t.pre_translate(34.0, 42.0).pre_concat(Transform::from_rotate(-38.0)));
    fill_path(pm, PathBuilder::from_circle(44.0, 35.5, 1.8), color(0xFFFFFF, 0.5), t);

    draw_sprout(pm, t, pose.leaf);
    draw_eyes(pm, t, pose);

    // Blush
    if pose.blush > 0.01 {
        for cx in [29.5, 70.5] {
            fill_path(pm, ellipse_path(cx, 71.5, 5.6, 3.3), color(0xFF8FA3, 0.55 * pose.blush), t);
        }
    }
    draw_mouth(pm, t, pose.mouth);
    draw_accessory(pm, base, t, pose);
}

fn draw_arm(pm: &mut PixmapMut, t: Transform, sx: f32, sy: f32, side: f32, raise: f32, c: u32) {
    // Rotate around the shoulder: 0 = resting at the side, 1 = raised up and out.
    // Positive rotation turns a hanging arm toward -x, so the left arm rotates positive.
    let angle = -side * (10.0 + 125.0 * raise.clamp(0.0, 1.2));
    let at = t.pre_translate(sx, sy).pre_concat(Transform::from_rotate(angle));
    fill_path(pm, ellipse_path(0.0, 7.0, 5.2, 8.5), color(c, 1.0), at);
}

fn draw_sprout(pm: &mut PixmapMut, t: Transform, leaf: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(50.0, 28.5);
    pb.quad_to(49.0, 22.0, 51.0, 17.5);
    stroke_path(pm, pb.finish(), color(LEAF, 1.0), 2.2, t);
    let leaf_path = || {
        let mut pb = PathBuilder::new();
        pb.move_to(0.0, 0.0);
        pb.quad_to(6.5, -6.5, 14.0, -0.5);
        pb.quad_to(7.0, 5.0, 0.0, 0.0);
        pb.close();
        pb.finish()
    };
    let top = t.pre_translate(51.0, 17.5);
    fill_path(pm, leaf_path(), color(LEAF, 1.0), top.pre_concat(Transform::from_rotate(-28.0 - leaf)));
    fill_path(pm, leaf_path(), color(mix(LEAF, 0xFFFFFF, 0.15), 1.0), top.pre_scale(-1.0, 1.0).pre_concat(Transform::from_rotate(-28.0 + leaf)));
}

fn draw_eyes(pm: &mut PixmapMut, t: Transform, p: &Pose) {
    let ox = 2.2 * p.look_x;
    let oy = 1.6 * p.look_y;
    for cx in [39.0, 61.0] {
        let (x, y) = (cx + ox, 61.0 + oy);
        let closed = p.blink > 0.85;
        match p.eyes {
            Eyes::Open | Eyes::Wide if !closed => {
                let (rx, ry) = if p.eyes == Eyes::Wide { (5.0, 6.3) } else { (4.3, 5.4) };
                let ry = (ry * (1.0 - p.blink)).max(0.7);
                fill_path(pm, ellipse_path(x, y, rx, ry), color(INK, 1.0), t);
                if ry > 2.5 {
                    fill_path(pm, PathBuilder::from_circle(x - 1.4, y - 2.0, 1.5), color(0xFFFFFF, 0.95), t);
                    fill_path(pm, PathBuilder::from_circle(x + 1.4, y + 1.7, 0.7), color(0xFFFFFF, 0.8), t);
                }
            }
            Eyes::Happy => {
                let mut pb = PathBuilder::new();
                pb.move_to(x - 4.2, y + 1.6);
                pb.quad_to(x, y - 4.2, x + 4.2, y + 1.6);
                stroke_path(pm, pb.finish(), color(INK, 1.0), 2.3, t);
            }
            Eyes::Sleepy => {
                fill_path(pm, ellipse_path(x, y + 1.3, 4.2, 2.5), color(INK, 1.0), t);
                let mut pb = PathBuilder::new();
                pb.move_to(x - 4.6, y - 0.2);
                pb.line_to(x + 4.6, y - 0.2);
                stroke_path(pm, pb.finish(), color(INK, 1.0), 1.6, t);
            }
            _ => {
                // Closed (or mid-blink): a gentle downward curve.
                let mut pb = PathBuilder::new();
                pb.move_to(x - 4.2, y - 0.4);
                pb.quad_to(x, y + 3.6, x + 4.2, y - 0.4);
                stroke_path(pm, pb.finish(), color(INK, 1.0), 2.1, t);
            }
        }
    }
}

fn draw_mouth(pm: &mut PixmapMut, t: Transform, m: Mouth) {
    let ink = color(INK, 1.0);
    let mut pb = PathBuilder::new();
    match m {
        Mouth::Smile => {
            pb.move_to(46.3, 70.0);
            pb.quad_to(50.0, 74.0, 53.7, 70.0);
            stroke_path(pm, pb.finish(), ink, 1.9, t);
        }
        Mouth::Grin => {
            pb.move_to(45.3, 69.4);
            pb.quad_to(50.0, 78.0, 54.7, 69.4);
            pb.close();
            fill_path(pm, pb.finish(), color(0x3B2530, 1.0), t);
            fill_path(pm, ellipse_path(50.0, 73.6, 2.4, 1.3), color(0xFF7F96, 1.0), t);
        }
        Mouth::O => fill_path(pm, ellipse_path(50.0, 72.0, 2.2, 2.8), color(0x3B2530, 1.0), t),
        Mouth::Wobble => {
            pb.move_to(45.5, 72.0);
            pb.quad_to(47.5, 70.2, 49.5, 72.0);
            pb.quad_to(51.5, 73.8, 54.0, 71.6);
            stroke_path(pm, pb.finish(), ink, 1.7, t);
        }
        Mouth::Tiny => {
            pb.move_to(48.4, 71.0);
            pb.quad_to(50.0, 72.4, 51.6, 71.0);
            stroke_path(pm, pb.finish(), ink, 1.6, t);
        }
    }
}

fn drop_path() -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(0.0, -7.5);
    pb.cubic_to(3.0, -3.5, 6.0, -0.5, 6.0, 3.0);
    pb.cubic_to(6.0, 6.6, 3.3, 8.8, 0.0, 8.8);
    pb.cubic_to(-3.3, 8.8, -6.0, 6.6, -6.0, 3.0);
    pb.cubic_to(-6.0, -0.5, -3.0, -3.5, 0.0, -7.5);
    pb.close();
    pb.finish()
}

pub fn draw_drop(pm: &mut PixmapMut, ts: Transform, fill: u32, alpha: f32) {
    fill_path(pm, drop_path(), color(fill, alpha), ts);
    fill_path(pm, ellipse_path(-2.2, 3.0, 1.4, 2.3), color(0xFFFFFF, 0.7 * alpha), ts);
}

fn heart_path() -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(0.0, 7.0);
    pb.cubic_to(-7.5, 1.5, -7.0, -5.5, -3.2, -5.5);
    pb.cubic_to(-1.2, -5.5, 0.0, -3.8, 0.0, -2.6);
    pb.cubic_to(0.0, -3.8, 1.2, -5.5, 3.2, -5.5);
    pb.cubic_to(7.0, -5.5, 7.5, 1.5, 0.0, 7.0);
    pb.close();
    pb.finish()
}

fn star_path(r: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(0.0, -r);
    pb.quad_to(0.0, 0.0, r, 0.0);
    pb.quad_to(0.0, 0.0, 0.0, r);
    pb.quad_to(0.0, 0.0, -r, 0.0);
    pb.quad_to(0.0, 0.0, 0.0, -r);
    pb.close();
    pb.finish()
}

fn draw_accessory(pm: &mut PixmapMut, base: Transform, body_t: Transform, p: &Pose) {
    let at = p.acc_t;
    match p.acc {
        Acc::None => {}
        Acc::Zzz => {
            for (i, (x, y, s)) in [(73.0, 31.0, 0.8), (80.0, 22.0, 1.05), (88.0, 12.0, 1.3)].iter().enumerate() {
                let mut pb = PathBuilder::new();
                let k = 3.4 * s;
                pb.move_to(-k, -k);
                pb.line_to(k, -k);
                pb.line_to(-k, k);
                pb.line_to(k, k);
                let alpha = 0.55 + 0.25 * i as f32 / 2.0;
                stroke_path(pm, pb.finish(), color(0x7A86A8, alpha), 1.7, base.pre_translate(*x, *y));
            }
        }
        Acc::Drop => {
            let bob = (at * 2.0 * PI).sin() * 1.5;
            draw_drop(pm, base.pre_translate(82.0, 28.0 + bob).pre_scale(0.95, 0.95), WATER, 1.0);
        }
        Acc::Alert => {
            let bob = (at * 2.0 * PI).sin() * 1.2;
            let b = base.pre_translate(82.0, 27.0 + bob);
            fill_path(pm, PathBuilder::from_circle(0.0, 0.0, 7.2), color(0xFFC53D, 1.0), b);
            fill_path(pm, super::rrect_path(-1.1, -4.3, 2.2, 5.4, 1.1), color(0x3B2A10, 1.0), b);
            fill_path(pm, PathBuilder::from_circle(0.0, 3.4, 1.2), color(0x3B2A10, 1.0), b);
        }
        Acc::Heart => {
            let s = 0.6 + 0.5 * (at * PI).sin();
            let b = base.pre_translate(78.0, 26.0 - 8.0 * at).pre_scale(s, s);
            fill_path(pm, heart_path(), color(0xFF6B8B, 1.0 - 0.6 * at.powi(3)), b);
        }
        Acc::Sparkles => {
            for (i, (x, y, r)) in [(20.0, 30.0, 4.5), (82.0, 24.0, 5.5), (88.0, 52.0, 3.5), (12.0, 55.0, 3.2)].iter().enumerate() {
                let tw = ((at * 3.0 + i as f32 * 0.37) * 2.0 * PI).sin() * 0.5 + 0.5;
                let c = if i % 2 == 0 { 0xFFC53D } else { 0xFFFFFF };
                fill_path(pm, star_path(r * (0.5 + 0.6 * tw)), color(c, 0.35 + 0.65 * tw), base.pre_translate(*x, *y));
            }
        }
        Acc::Sip => {
            // From above-right down into the mouth, shrinking as it goes.
            let k = at;
            let x = 82.0 + (50.0 - 82.0) * k;
            let y = 22.0 + (71.0 - 22.0) * (k * k);
            let s = 1.0 - 0.6 * k;
            draw_drop(pm, body_t.pre_translate(x, y).pre_scale(s, s), WATER, 1.0);
        }
    }
}

pub fn draw_drop_outline(pm: &mut PixmapMut, ts: Transform, stroke: u32) {
    stroke_path(pm, drop_path(), color(stroke, 1.0), 1.6, ts);
}
