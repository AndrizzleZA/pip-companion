//! Renders the pictures in the README from the drawing code itself.
//!
//! Run from the repository root:
//!
//! ```text
//! cargo run -p pip-draw --example shots --features png-format
//! ```
//!
//! The point of generating them rather than taking them is that they cannot drift. A screenshot of
//! a creature is a claim about what the creature looks like, and this project has had to correct
//! four of those claims in a fortnight (§51, §53, §54). These are produced by the same
//! `creature::draw` the desktop and the phone call, so a picture that is wrong means the program
//! is wrong, which is the only kind of wrong worth having.

use pip_core::settings::{Character, PetColor};
use pip_draw::creature::{self, AnimKind, Expr, Pose};
use pip_draw::{color, tiny_skia::*};

/// The creature's box, matching the desktop window (`pet.rs`).
const BOX_W: f32 = 112.0;
const BOX_H: f32 = 104.0;
const UNIT: f32 = 0.92;
/// Big enough to stay crisp on a high-DPI screen without becoming a large file.
const SCALE: f32 = 2.0;
/// The app's own light surface, so the pictures look like the thing they are of.
const BG: u32 = 0xF4F5F8;

fn cell(w: u32, h: u32) -> Pixmap {
    let mut pm = Pixmap::new(w, h).expect("a pixmap");
    pm.fill(color(BG, 1.0));
    pm
}

/// Draws one creature into `pm` at cell `i`, with an optional animation frame.
fn one(pm: &mut Pixmap, i: u32, who: Character, body: u32, pose: &Pose) {
    let cw = (BOX_W * SCALE) as u32;
    let base = Transform::from_translate((i * cw) as f32, 0.0)
        .pre_scale(SCALE, SCALE)
        .pre_translate((BOX_W - 100.0 * UNIT) / 2.0, BOX_H - 4.0 - creature::GROUND_Y * UNIT)
        .pre_scale(UNIT, UNIT);
    creature::draw(&mut pm.as_mut(), pose, body, who, base);
}

fn strip(n: u32) -> Pixmap {
    cell((BOX_W * SCALE) as u32 * n, (BOX_H * SCALE) as u32)
}

/// Trims the empty background down to a even margin.
///
/// The creature stands on the bottom of its window and the space above it belongs to whatever
/// crown it happens to have, so a fixed crop would be wrong for every character but one. Finding
/// the ink and cropping to it keeps the pictures tight whoever is in them, including the ones
/// nobody has drawn yet.
fn trim(pm: &Pixmap, margin: u32) -> Pixmap {
    let (w, h) = (pm.width(), pm.height());
    let bg = color(BG, 1.0).to_color_u8();
    let px = pm.pixels();
    let mut top = h;
    let mut bottom = 0u32;
    for y in 0..h {
        for x in 0..w {
            let p = px[(y * w + x) as usize];
            // Demultiplied comparison is unnecessary here: the background is opaque, so anything
            // that differs from it at all is something drawn on top.
            if p.red() != bg.red() || p.green() != bg.green() || p.blue() != bg.blue() {
                top = top.min(y);
                bottom = bottom.max(y);
                break;
            }
        }
    }
    if top > bottom {
        return pm.clone(); // nothing drawn; leave it alone rather than crop to nothing
    }
    let y0 = top.saturating_sub(margin);
    let y1 = (bottom + margin + 1).min(h);
    let mut out = Pixmap::new(w, y1 - y0).expect("a pixmap");
    out.fill(color(BG, 1.0));
    out.draw_pixmap(0, -(y0 as i32), pm.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    out
}

fn save(pm: &Pixmap, name: &str) {
    let pm = trim(pm, (10.0 * SCALE) as u32);
    let path = std::path::Path::new("docs").join(name);
    pm.save_png(&path).expect("could not write the picture");
    println!("{} ({} x {})", path.display(), pm.width(), pm.height());
}

fn main() {
    // The whole cast, one colour each, so the picture doubles as the palette.
    let mut cast = strip(Character::ALL.len() as u32);
    for (i, (who, colour)) in Character::ALL.iter().zip(PetColor::ALL).enumerate() {
        let mut pose = Pose::base(Expr::Happy);
        pose.blush = 0.6;
        one(&mut cast, i as u32, *who, colour.rgb(), &pose);
    }
    save(&cast, "cast.png");

    // Growing up: the same companion, newly arrived, half grown, grown.
    let mut growth = strip(3);
    for (i, stage) in [0u8, 1, 2].iter().enumerate() {
        let mut pose = Pose::base(Expr::Happy);
        pose.stage = *stage;
        one(&mut growth, i as u32, Character::Sprout, PetColor::Mint.rgb(), &pose);
    }
    save(&growth, "growing-up.png");

    // Hiding: the dig, at the moments that say what is happening.
    let moments = [0.18, 0.34, 0.50, 0.62, 0.74, 0.92];
    let mut dig = strip(moments.len() as u32);
    for (i, t) in moments.iter().enumerate() {
        let mut pose = Pose::base(Expr::Content);
        creature::animate(AnimKind::Burrow, *t, &mut pose);
        one(&mut dig, i as u32, Character::Sprout, PetColor::Mint.rgb(), &pose);
    }
    save(&dig, "burrow.png");

    // Quiet hours: asleep in the burrow, which is where it spends the night.
    let mut night = strip(1);
    let mut pose = Pose::base(Expr::Asleep);
    creature::nestled(&mut pose);
    one(&mut night, 0, Character::Bunny, PetColor::Lavender.rgb(), &pose);
    save(&night, "night.png");
}
