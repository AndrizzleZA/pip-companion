//! The creature, drawn procedurally from a handful of pose parameters.
//!
//! Drawing is vector-based in a 100×100 design space and scaled at render time, so it is crisp on
//! every DPI and recolouring costs nothing. Animations are pure functions of (kind, t) that tweak a
//! `Pose`; the window only renders frames while an animation is actually playing.

use super::{color, ellipse_path, fill_path, mix, stroke_path};
use pip_core::settings::Character;
use std::f32::consts::PI;
use tiny_skia::*;

/// Where the feet touch the ground in design space.
pub const GROUND_Y: f32 = 94.0;
/// Everything is drawn symmetrically about this line.
const CX: f32 = 50.0;
/// The mouth shapes are authored around this height; a body moves them by shifting the transform
/// rather than by restating five sets of curves.
const MOUTH_BASE: f32 = 71.0;

/// How a companion looks at each stage of growing up.
///
/// Four things move together on purpose. Size alone was too easy to miss: a creature a hundred
/// pixels tall losing a sixth of its height reads as "drawn slightly smaller", not as "younger".
/// The crown carries most of the legibility, because it is each character's silhouette — a stub of
/// a sprout, half-grown ears — and the eyes carry the rest, because big eyes in a small face is
/// the thing everyone reads as young without being told.
struct Growth {
    scale: f32,
    /// Eye size relative to the body.
    eye: f32,
    /// Sprout, ears or tuft, as a fraction of their grown size.
    crown: f32,
    /// Wider and shorter when young.
    squat: f32,
}

fn growth(stage: u8) -> Growth {
    match stage {
        0 => Growth { scale: 0.78, eye: 1.22, crown: 0.40, squat: 1.08 },
        1 => Growth { scale: 0.90, eye: 1.10, crown: 0.72, squat: 1.03 },
        _ => Growth { scale: 1.0, eye: 1.0, crown: 1.0, squat: 1.0 },
    }
}

/// What sits on a head. Separate from the body so the two multiply: a future creature is any
/// body with any crown, which is one more line in [`shape_of`] rather than more drawing code.
#[derive(Clone, Copy, PartialEq)]
pub enum Crown {
    Leaf,
    /// Tall, soft ears — drawn behind the body so the head overlaps their base.
    LongEars,
    /// Pointed ears, also behind. Nothing wears these since Kit became a mushroom; it stays
    /// because crowns are a library that bodies multiply with, and the next creature that wants
    /// pointed ears should find them already drawn rather than draw them again.
    #[allow(dead_code)]
    CatEars,
    Tuft,
    /// Nothing at all: a smooth head, distinct by absence.
    None,
    /// A wide domed cap with a flat underside. On top, and much wider than the body it sits on.
    Cap,
    /// A ring of small petals around the crown point. On top, so they overlap the head.
    Petals,
    /// Two thin jointed stalks with a knob on each tip.
    Antennae,
}

impl Crown {
    /// Ears sit behind the body; everything else goes on top of it.
    fn behind(self) -> bool {
        matches!(self, Crown::LongEars | Crown::CatEars)
    }
}

/// A body outline: a symmetric blob, described by where it is widest and how far its controls
/// reach. These eight numbers reproduce the original drawing exactly, which is what makes it safe
/// to turn a hand-drawn shape into data.
#[derive(Clone, Copy)]
pub struct Body {
    pub top: f32,
    pub bottom: f32,
    /// Half-width at the widest point.
    pub half: f32,
    /// Height of the widest point.
    pub widest: f32,
    /// How far the top and bottom controls reach sideways.
    pub shoulder: f32,
    pub hip: f32,
    /// Where the side controls sit, above and below the widest point.
    pub flank_hi: f32,
    pub flank_lo: f32,
}

impl Body {
    fn path(&self) -> Option<Path> {
        let (t, b, h, w) = (self.top, self.bottom, self.half, self.widest);
        let mut pb = PathBuilder::new();
        pb.move_to(CX, t);
        pb.cubic_to(CX + self.shoulder, t, CX + h, self.flank_hi, CX + h, w);
        pb.cubic_to(CX + h, self.flank_lo, CX + self.hip, b, CX, b);
        pb.cubic_to(CX - self.hip, b, CX - h, self.flank_lo, CX - h, w);
        pb.cubic_to(CX - h, self.flank_hi, CX - self.shoulder, t, CX, t);
        pb.close();
        pb.finish()
    }
}

/// Where everything hangs off a body. The poses, expressions and animations are shared, so a new
/// creature is this many numbers rather than another copy of [`draw`].
#[derive(Clone, Copy)]
pub struct Shape {
    pub body: Body,
    pub crown: Crown,
    pub eye_y: f32,
    /// Half the distance between the eyes.
    pub eye_dx: f32,
    /// Where the authored mouth lands on this face.
    pub mouth_y: f32,
    /// Belly patch: (centre y, rx, ry).
    pub belly: (f32, f32, f32),
    pub shine: (f32, f32),
    pub shine2: (f32, f32),
    /// Blush: (offset from centre, y).
    pub blush: (f32, f32),
    /// Shoulders: (offset from centre, y).
    pub arm: (f32, f32),
    /// Feet: (offset from centre, y).
    pub foot: (f32, f32),
    pub shadow_rx: f32,
    /// Where a crown sits on the head.
    pub crown_y: f32,
    /// Where ears attach: (offset from centre, y).
    pub ear: (f32, f32),
    /// Where a floating accessory hovers.
    pub acc: (f32, f32),
}

/// The four creatures. Sprout's numbers are the original drawing, unchanged to the decimal.
pub fn shape_of(who: Character) -> Shape {
    match who {
        Character::Sprout => Shape {
            crown: Crown::Leaf,
            body: Body { top: 27.0, bottom: 93.0, half: 35.0, widest: 66.0, shoulder: 22.0, hip: 21.0, flank_hi: 47.0, flank_lo: 85.0 },
            eye_y: 61.0,
            eye_dx: 11.0,
            mouth_y: 71.0,
            belly: (77.0, 19.0, 12.5),
            shine: (34.0, 42.0),
            shine2: (44.0, 35.5),
            blush: (20.5, 71.5),
            arm: (29.0, 63.0),
            foot: (12.0, 91.0),
            shadow_rx: 25.0,
            crown_y: 28.5,
            ear: (10.0, 32.0),
            acc: (82.0, 27.0),
        },
        // Tall and slim: a taller head means the face rides higher and the feet sit closer in.
        Character::Bunny => Shape {
            crown: Crown::LongEars,
            body: Body { top: 19.0, bottom: 93.0, half: 28.0, widest: 65.0, shoulder: 17.0, hip: 17.0, flank_hi: 40.0, flank_lo: 84.0 },
            eye_y: 56.0,
            eye_dx: 9.5,
            mouth_y: 66.0,
            belly: (79.0, 15.5, 11.0),
            shine: (37.0, 36.0),
            shine2: (45.0, 30.0),
            blush: (17.0, 66.5),
            arm: (23.0, 61.0),
            foot: (10.0, 91.0),
            shadow_rx: 21.0,
            crown_y: 20.5,
            ear: (9.0, 25.0),
            acc: (76.0, 20.0),
        },
        // A stem under a wide cap. The body is narrow and flares at the foot — the widest point is
        // almost at the ground — so the cap is what gives this one its width. The shadow is far
        // wider than half × 0.72 because the cap overhangs the stem by more than two to one.
        Character::Kit => Shape {
            crown: Crown::Cap,
            body: Body { top: 50.0, bottom: 93.0, half: 20.0, widest: 86.0, shoulder: 12.0, hip: 17.0, flank_hi: 66.0, flank_lo: 91.0 },
            eye_y: 68.0,
            eye_dx: 6.5,
            mouth_y: 78.0,
            belly: (84.0, 11.0, 8.0),
            shine: (41.0, 62.0),
            shine2: (46.0, 57.0),
            blush: (12.0, 78.5),
            arm: (16.5, 72.0),
            foot: (7.0, 91.0),
            shadow_rx: 32.0,
            crown_y: 51.0,
            ear: (10.0, 44.0),
            acc: (68.0, 50.0),
        },
        // Narrow-shouldered and bottom-heavy: slim, but it sits into the ground rather than
        // standing tall. Bunny is already the slim one, so narrowing alone would only trade one
        // collision for another; what no other creature has is a low widest point with the
        // shoulders pulled well inside the hips.
        Character::Tuft => Shape {
            crown: Crown::Tuft,
            body: Body { top: 28.0, bottom: 93.0, half: 26.0, widest: 70.0, shoulder: 13.0, hip: 19.0, flank_hi: 46.0, flank_lo: 86.0 },
            eye_y: 60.0,
            eye_dx: 9.0,
            mouth_y: 70.0,
            belly: (79.0, 14.5, 11.0),
            shine: (38.0, 43.0),
            shine2: (45.0, 37.0),
            blush: (15.5, 70.5),
            arm: (21.5, 63.0),
            foot: (9.0, 91.0),
            shadow_rx: 19.0,
            crown_y: 29.0,
            ear: (8.5, 33.0),
            acc: (74.0, 28.0),
        },
        // Broad, low and smooth-headed. It takes the wide silhouette the mushroom gave up, and
        // with no crown at all the outline is the entire character — so it has to be the widest
        // thing in the set or there is nothing to recognise it by.
        Character::Pebble => Shape {
            crown: Crown::None,
            body: Body { top: 38.0, bottom: 93.0, half: 43.0, widest: 74.0, shoulder: 27.0, hip: 26.0, flank_hi: 54.0, flank_lo: 86.0 },
            eye_y: 66.0,
            eye_dx: 14.0,
            mouth_y: 76.0,
            belly: (83.0, 23.5, 10.5),
            shine: (30.0, 51.0),
            shine2: (42.0, 46.0),
            blush: (25.5, 76.5),
            arm: (36.0, 69.0),
            foot: (15.0, 91.0),
            shadow_rx: 31.0,
            crown_y: 39.0,
            ear: (16.0, 45.0),
            acc: (91.0, 38.0),
        },
        // A small round body under a wide ring of petals, so the crown carries the shape rather
        // than the outline. The shadow is wider than half × 0.72 on purpose: the petals overhang.
        Character::Bloom => Shape {
            crown: Crown::Petals,
            body: Body { top: 40.0, bottom: 93.0, half: 27.0, widest: 72.0, shoulder: 17.0, hip: 17.0, flank_hi: 56.0, flank_lo: 86.0 },
            eye_y: 64.0,
            eye_dx: 9.0,
            mouth_y: 74.0,
            belly: (79.0, 15.0, 10.5),
            shine: (37.0, 54.0),
            shine2: (45.0, 48.0),
            blush: (16.0, 74.5),
            arm: (22.5, 67.0),
            foot: (9.5, 91.0),
            shadow_rx: 24.0,
            crown_y: 41.0,
            ear: (10.0, 44.0),
            acc: (75.0, 40.0),
        },
        // The smallest of the set: a low, narrow body that leaves the antennae room to read.
        Character::Moth => Shape {
            crown: Crown::Antennae,
            body: Body { top: 44.0, bottom: 93.0, half: 24.0, widest: 74.0, shoulder: 15.0, hip: 16.0, flank_hi: 58.0, flank_lo: 87.0 },
            eye_y: 66.0,
            eye_dx: 8.0,
            mouth_y: 76.0,
            belly: (79.0, 13.0, 10.0),
            shine: (39.0, 58.0),
            shine2: (45.5, 52.0),
            blush: (14.5, 76.5),
            arm: (20.0, 69.0),
            foot: (8.5, 91.0),
            shadow_rx: 17.0,
            crown_y: 45.0,
            ear: (9.0, 48.0),
            acc: (72.0, 44.0),
        },
    }
}

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
    /// 0 newly arrived, 1 growing up, 2 fully grown. Defaults to grown, so anything drawing a
    /// portrait — the tray icon — shows the companion as it will be.
    pub stage: u8,
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
            stage: 2,
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
/// The mushroom cap and its spots.
const CAP: u32 = 0xD4695C;
const CAP_SPOT: u32 = 0xFBF0E4;
/// Bloom's petals and the dot of pollen at their centre.
const PETAL: u32 = 0xF3B9D4;
const POLLEN: u32 = 0xF7D774;
const WATER: u32 = 0x4DB3F2;

/// Draws the creature. `base` maps design space (0..100) into the target pixmap.
pub fn draw(pm: &mut PixmapMut, pose: &Pose, body: u32, who: Character, base: Transform) {
    let body_light = mix(body, 0xFFFFFF, 0.38);
    let body_dark = mix(body, 0x000000, 0.18);
    let s = shape_of(who);

    // Ground shadow: stays on the ground, shrinks while airborne.
    let lift = (-pose.dy).max(0.0);
    let sh = (1.0 - lift / 55.0).clamp(0.4, 1.0);
    let g = growth(pose.stage);
    fill_path(
        pm,
        ellipse_path(CX, GROUND_Y + 0.5, s.shadow_rx * sh * pose.sx * g.scale * g.squat, 3.6 * sh),
        color(0, 0.16 * sh),
        base,
    );

    // Body transform: squash/stretch, tilt and growing up, all around the feet.
    let t = base
        .pre_translate(CX, GROUND_Y + pose.dy)
        .pre_concat(Transform::from_rotate(pose.tilt))
        .pre_scale(pose.sx * g.scale * g.squat, pose.sy * g.scale / g.squat)
        .pre_translate(-CX, -GROUND_Y);
    // Crowns grow on their own curve, about the top of the head.
    //
    // A cap gets a floor, because it is the only crown wider than the body under it: at 0.40 it
    // shrinks past its own stem and reads as a red band around the head rather than a mushroom.
    // Everything else is narrower than its head and only looks younger as it shrinks, so the floor
    // is here rather than in the growth table, which the whole cast shares.
    let crown = if s.crown == Crown::Cap { g.crown.max(0.70) } else { g.crown };
    let ct = t
        .pre_translate(CX, s.crown_y)
        .pre_scale(crown, crown)
        .pre_translate(-CX, -s.crown_y);

    // Feet
    for side in [-1.0f32, 1.0] {
        fill_path(pm, ellipse_path(CX + side * s.foot.0, s.foot.1 + pose.feet_drop, 8.0, 4.6), color(body_dark, 1.0), t);
    }

    // Ears belong behind the body, like the arms, so the head overlaps them cleanly.
    draw_ears_behind(pm, ct, &s, body, pose.leaf);

    // Arms (behind the body so they peek out as little nubs)
    draw_arm(pm, t, CX - s.arm.0, s.arm.1, -1.0, pose.arm_l, body_dark);
    draw_arm(pm, t, CX + s.arm.0, s.arm.1, 1.0, pose.arm_r, body_dark);

    // Body: a soft mochi shape with a vertical gradient.
    if let Some(path) = s.body.path() {
        let shader = LinearGradient::new(
            Point::from_xy(CX, s.body.top),
            Point::from_xy(CX, s.body.bottom),
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
    fill_path(pm, ellipse_path(CX, s.belly.0, s.belly.1, s.belly.2), color(mix(body, 0xFFFFFF, 0.5), 0.5), t);
    fill_path(pm, ellipse_path(0.0, 0.0, 7.5, 4.0), color(0xFFFFFF, 0.6), t.pre_translate(s.shine.0, s.shine.1).pre_concat(Transform::from_rotate(-38.0)));
    fill_path(pm, PathBuilder::from_circle(s.shine2.0, s.shine2.1, 1.8), color(0xFFFFFF, 0.5), t);

    draw_crown(pm, ct, &s, pose.leaf);
    draw_eyes(pm, t, &s, pose);

    // Blush
    if pose.blush > 0.01 {
        for side in [-1.0f32, 1.0] {
            fill_path(pm, ellipse_path(CX + side * s.blush.0, s.blush.1, 5.6, 3.3), color(0xFF8FA3, 0.55 * pose.blush), t);
        }
    }
    // The mouth art is authored once and moved to this face, rather than redrawn per creature.
    draw_mouth(pm, t.pre_translate(0.0, s.mouth_y - MOUTH_BASE), pose.mouth);
    draw_accessory(pm, base, t, &s, pose);
}

fn draw_arm(pm: &mut PixmapMut, t: Transform, sx: f32, sy: f32, side: f32, raise: f32, c: u32) {
    // Rotate around the shoulder: 0 = resting at the side, 1 = raised up and out.
    // Positive rotation turns a hanging arm toward -x, so the left arm rotates positive.
    let angle = -side * (10.0 + 125.0 * raise.clamp(0.0, 1.2));
    let at = t.pre_translate(sx, sy).pre_concat(Transform::from_rotate(angle));
    fill_path(pm, ellipse_path(0.0, 7.0, 5.2, 8.5), color(c, 1.0), at);
}

/// What sits on top: the one thing that differs between characters. `sway` is the same gentle
/// value the sprout already used, so every crown moves with the same breathing.
fn draw_crown(pm: &mut PixmapMut, t: Transform, s: &Shape, sway: f32) {
    match s.crown {
        Crown::Leaf => draw_sprout(pm, t, s.crown_y, sway),
        Crown::Tuft => draw_tuft(pm, t, s.crown_y, sway),
        // Nothing to draw: Pebble is bald on purpose.
        Crown::None => {}
        Crown::Cap => draw_cap(pm, t, s.crown_y, sway),
        Crown::Petals => draw_petals(pm, t, s.crown_y, sway),
        Crown::Antennae => draw_antennae(pm, t, s.crown_y, sway),
        // Ears were already drawn, behind the body.
        c if c.behind() => {}
        _ => {}
    }
}

/// Bunny and Kit ears, drawn before the body so the head overlaps their base.
fn draw_ears_behind(pm: &mut PixmapMut, t: Transform, s: &Shape, body: u32, sway: f32) {
    if !s.crown.behind() {
        return;
    }
    let shell = mix(body, 0xFFFFFF, 0.1);
    let inner = mix(body, 0xFF8FA3, 0.45);
    match s.crown {
        Crown::LongEars => {
            for dir in [-1.0f32, 1.0] {
                let et = t
                    .pre_translate(CX + dir * s.ear.0, s.ear.1)
                    .pre_concat(Transform::from_rotate(dir * (9.0 + sway * 0.5)));
                fill_path(pm, ellipse_path(0.0, -13.0, 5.4, 15.0), color(shell, 1.0), et);
                fill_path(pm, ellipse_path(0.0, -13.0, 2.6, 10.5), color(inner, 0.8), et);
            }
        }
        Crown::CatEars => {
            for dir in [-1.0f32, 1.0] {
                let et = t
                    .pre_translate(CX + dir * s.ear.0, s.ear.1)
                    .pre_concat(Transform::from_rotate(dir * (8.0 + sway * 0.4)));
                let mut pb = PathBuilder::new();
                pb.move_to(-7.0, 2.0);
                pb.line_to(0.0, -14.0);
                pb.line_to(7.0, 2.0);
                pb.close();
                fill_path(pm, pb.finish(), color(shell, 1.0), et);
                let mut ib = PathBuilder::new();
                ib.move_to(-3.4, 0.5);
                ib.line_to(0.0, -8.5);
                ib.line_to(3.4, 0.5);
                ib.close();
                fill_path(pm, ib.finish(), color(inner, 0.75), et);
            }
        }
        _ => {}
    }
}

/// Three short strands, leaning with the same sway as the sprout.
fn draw_tuft(pm: &mut PixmapMut, t: Transform, crown_y: f32, sway: f32) {
    let top = t.pre_translate(CX, crown_y - 0.5);
    for (dx, len, lean) in [(-4.5f32, 9.0f32, -24.0f32), (0.0, 12.0, -4.0), (4.5, 9.0, 16.0)] {
        let mut pb = PathBuilder::new();
        pb.move_to(0.0, 0.0);
        pb.quad_to(dx * 0.4, -len * 0.6, dx * 0.9, -len);
        stroke_path(pm, pb.finish(), color(INK, 0.85), 2.0, top.pre_concat(Transform::from_rotate(lean + sway * 0.6)));
    }
}

/// A wide domed cap with a flat underside, and three lighter spots.
///
/// Flat underneath rather than a full ellipse: a ball on a stem reads as a lollipop, and the one
/// line that makes it a mushroom is the straight edge where the cap meets the stem.
fn draw_cap(pm: &mut PixmapMut, t: Transform, crown_y: f32, sway: f32) {
    let top = t.pre_translate(CX, crown_y).pre_concat(Transform::from_rotate(sway * 0.35));
    const HALF: f32 = 44.0;
    const RISE: f32 = 22.0;
    let mut pb = PathBuilder::new();
    pb.move_to(-HALF, 0.0);
    pb.cubic_to(-HALF, -RISE * 0.95, -HALF * 0.45, -RISE, 0.0, -RISE);
    pb.cubic_to(HALF * 0.45, -RISE, HALF, -RISE * 0.95, HALF, 0.0);
    // The underside lifts very slightly at the middle so the cap sits on the stem rather than
    // being speared by it.
    pb.quad_to(0.0, -2.6, -HALF, 0.0);
    pb.close();
    fill_path(pm, pb.finish(), color(CAP, 1.0), top);
    for (dx, dy, r) in [(-19.0f32, -9.0f32, 5.2f32), (6.0, -13.5, 4.0), (22.0, -6.5, 4.6)] {
        fill_path(pm, ellipse_path(dx, dy, r, r * 0.86), color(CAP_SPOT, 0.9), top);
    }
}

/// A ring of petals, drawn as filled circles about the crown point.
///
/// Circles rather than strokes because the whole crown is scaled to 0.40 at the first stage: a
/// stroked outline thins to under a pixel and greys out, while a filled shape only gets smaller.
fn draw_petals(pm: &mut PixmapMut, t: Transform, crown_y: f32, sway: f32) {
    // Above the head, not on it. Sitting it at the crown point made a badge on the forehead; the
    // flower has to be wider than the head it sits on or Bloom is just a round body with a sticker.
    let top = t.pre_translate(CX, crown_y - 3.0).pre_concat(Transform::from_rotate(sway * 0.5));
    const PETALS: usize = 6;
    for i in 0..PETALS {
        let a = (i as f32 / PETALS as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let (x, y) = (a.cos() * 11.0, a.sin() * 11.0);
        fill_path(pm, ellipse_path(x, y, 6.2, 6.2), color(PETAL, 1.0), top);
    }
    fill_path(pm, ellipse_path(0.0, 0.0, 4.4, 4.4), color(POLLEN, 1.0), top);
}

/// Two jointed stalks with a knob on each tip.
///
/// The knobs are what make this survive growing up: at 0.40 the stalks are under a pixel wide and
/// nearly gone, and without something solid on the end a baby Moth would read as having nothing on
/// its head at all.
fn draw_antennae(pm: &mut PixmapMut, t: Transform, crown_y: f32, sway: f32) {
    let top = t.pre_translate(CX, crown_y - 0.5);
    for dir in [-1.0f32, 1.0] {
        let at = top.pre_concat(Transform::from_rotate(dir * 24.0 + sway * 0.6));
        let mut pb = PathBuilder::new();
        pb.move_to(0.0, 0.0);
        pb.quad_to(dir * 3.0, -7.0, dir * 4.5, -13.0);
        stroke_path(pm, pb.finish(), color(INK, 0.85), 2.8, at);
        fill_path(pm, ellipse_path(dir * 4.5, -13.0, 3.3, 3.3), color(INK, 0.85), at);
    }
}

fn draw_sprout(pm: &mut PixmapMut, t: Transform, crown_y: f32, leaf: f32) {
    // Authored against Sprout's own head; another head just moves the whole stem.
    let t = t.pre_translate(0.0, crown_y - 28.5);
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

fn draw_eyes(pm: &mut PixmapMut, t: Transform, s: &Shape, p: &Pose) {
    let ox = 2.2 * p.look_x;
    let oy = 1.6 * p.look_y;
    // Big eyes in a small face: the cue that says young rather than merely small.
    let eye = growth(p.stage).eye;
    for side in [-1.0f32, 1.0] {
        let (x, y) = (CX + side * s.eye_dx + ox, s.eye_y + oy);
        let closed = p.blink > 0.85;
        match p.eyes {
            Eyes::Open | Eyes::Wide if !closed => {
                let (rx, ry) = if p.eyes == Eyes::Wide { (5.0, 6.3) } else { (4.3, 5.4) };
                let (rx, ry) = (rx * eye, ry * eye);
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

fn draw_accessory(pm: &mut PixmapMut, base: Transform, body_t: Transform, s: &Shape, p: &Pose) {
    let at = p.acc_t;
    // Badges hover beside the head, so they move with it; Sprout's numbers are the baseline.
    let (adx, ady) = (s.acc.0 - 82.0, s.acc.1 - 27.0);
    let base = base.pre_translate(adx, ady);
    // Sparkles ring the body, so they move out with a wider one.
    let spread = s.body.half - 35.0;
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
                let x = if *x < CX { x - spread } else { x + spread };
                fill_path(pm, star_path(r * (0.5 + 0.6 * tw)), color(c, 0.35 + 0.65 * tw), base.pre_translate(x, *y));
            }
        }
        Acc::Sip => {
            // From above-right down into the mouth, shrinking as it goes.
            let k = at;
            let (mx, my) = (CX, s.mouth_y);
            let x = 82.0 + (mx - 82.0) * k;
            let y = 22.0 + (my - 22.0) * (k * k);
            let s = 1.0 - 0.6 * k;
            draw_drop(pm, body_t.pre_translate(x, y).pre_scale(s, s), WATER, 1.0);
        }
    }
}

pub fn draw_drop_outline(pm: &mut PixmapMut, ts: Transform, stroke: u32) {
    stroke_path(pm, drop_path(), color(stroke, 1.0), 1.6, ts);
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    /// Turning a hand-drawn body into eight numbers is only safe if the numbers give back exactly
    /// the same curve. A screenshot can't prove this — the pet blinks, so two captures of one
    /// build differ — but the path can be compared point by point.
    #[test]
    fn sprouts_body_is_the_original_drawing_to_the_point() {
        let mut pb = PathBuilder::new();
        pb.move_to(50.0, 27.0);
        pb.cubic_to(72.0, 27.0, 85.0, 47.0, 85.0, 66.0);
        pb.cubic_to(85.0, 85.0, 71.0, 93.0, 50.0, 93.0);
        pb.cubic_to(29.0, 93.0, 15.0, 85.0, 15.0, 66.0);
        pb.cubic_to(15.0, 47.0, 28.0, 27.0, 50.0, 27.0);
        pb.close();
        let original = pb.finish().expect("the drawing as it was written by hand");
        let from_plan = shape_of(Character::Sprout).body.path().expect("the same body, as data");

        let a: Vec<String> = original.segments().map(|s| format!("{s:?}")).collect();
        let b: Vec<String> = from_plan.segments().map(|s| format!("{s:?}")).collect();
        assert_eq!(a, b, "Sprout's outline changed");
    }

    /// The face and limbs were absolute numbers before; the plan has to put them back where they
    /// were, or Sprout's expression shifts under everyone who never asked for a new creature.
    #[test]
    fn sprouts_anchors_are_where_they_always_were() {
        let s = shape_of(Character::Sprout);
        assert_eq!((CX - s.eye_dx, CX + s.eye_dx, s.eye_y), (39.0, 61.0, 61.0), "eyes");
        assert_eq!(s.mouth_y, MOUTH_BASE, "the mouth is not moved at all");
        assert_eq!((CX - s.arm.0, CX + s.arm.0, s.arm.1), (21.0, 79.0, 63.0), "shoulders");
        assert_eq!((CX - s.foot.0, CX + s.foot.0, s.foot.1), (38.0, 62.0, 91.0), "feet");
        assert_eq!((CX - s.blush.0, CX + s.blush.0, s.blush.1), (29.5, 70.5, 71.5), "blush");
        assert_eq!((s.belly.0, s.belly.1, s.belly.2), (77.0, 19.0, 12.5), "belly");
        assert_eq!(s.shine, (34.0, 42.0));
        assert_eq!(s.shine2, (44.0, 35.5));
        assert_eq!(s.shadow_rx, 25.0);
        assert_eq!(s.acc, (82.0, 27.0), "accessories hover where they did");
        assert_eq!(s.body.half, 35.0, "so sparkles spread by zero");
    }

    /// Every creature has to fit the window, which also fixes hit-testing and the saved position.
    /// Usable design space is x -10.9..110.9 and y -14.7..98.3 (BOX 112x104, UNIT 0.92).
    #[test]
    fn every_creature_fits_the_window() {
        for who in Character::ALL {
            let s = shape_of(who);
            let b = s.body.path().expect("a body").bounds();
            let reach = |v: f32| assert!((-10.9..=110.9).contains(&v), "{:?} is off the side at {v}", who);
            reach(b.left() - 12.0); // feet and ears sit outside the outline
            reach(b.right() + 12.0);
            assert!(b.top() > -14.7, "{who:?} is clipped at the top");
            assert!(b.bottom() <= 98.3, "{who:?} is clipped at the bottom");
            assert!(s.body.bottom == 93.0, "{who:?} must stand on the same ground");
        }
    }
}
