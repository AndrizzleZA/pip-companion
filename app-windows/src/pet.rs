//! The creature's window: a per-pixel-alpha layered popup that never takes focus.
//!
//! This module is the *view*: layout, drawing, hit-testing and the short-lived animation clock.
//! Decisions (what to show, when) live in `app.rs`.

use crate::platform::{self, tick_ms, IRect};
use crate::render::creature::{self, Acc, AnimKind, Expr, Pose};
use crate::render::text::{self, fonts};
use crate::render::ui::{self, BtnStyle, R};
use crate::render::{color, fill_path, fill_rrect, soft_shadow, stroke_rrect, Palette};
use crate::surface::Surface;
use pip_core::settings::Character;
use tiny_skia::{PathBuilder, PixmapMut, Transform};
use windows_sys::Win32::Foundation::HWND;

/// Creature box in logical pixels (the pet occupies this even when no bubble shows).
pub const BOX_W: f32 = 112.0;
pub const BOX_H: f32 = 104.0;
/// Design units (0..100) → logical px, and where design (0,0) sits inside the box.
const UNIT: f32 = 0.92;
const ORIGIN_X: f32 = (BOX_W - 100.0 * UNIT) / 2.0;
const ORIGIN_Y: f32 = BOX_H - 4.0 - creature::GROUND_Y * UNIT;
const MAX_CARD_W: f32 = 264.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Done,
    Snooze,
    Dismiss,
    Drink,
    Settings,
    Close,
    /// Open the thing in its app (Gmail).
    Open,
    /// "Always from this sender": adds the one email's sender to the important list.
    AlwaysFrom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BubbleKind {
    Tooltip,
    Info,
    Notify,
    Message,
}

#[derive(Clone, Debug)]
pub struct Line {
    pub text: String,
    pub muted: bool,
}

#[derive(Clone, Debug)]
pub struct Card {
    pub kind: BubbleKind,
    pub big: Option<String>,
    pub title: Option<String>,
    pub lines: Vec<Line>,
    pub water: Option<(u32, u32)>,
    pub buttons: Vec<(String, Action, BtnStyle)>,
    /// Accent strip on the left (important reminders).
    pub important: bool,
}

impl Card {
    pub fn new(kind: BubbleKind) -> Card {
        Card { kind, big: None, title: None, lines: Vec::new(), water: None, buttons: Vec::new(), important: false }
    }
    pub fn line(mut self, text: impl Into<String>, muted: bool) -> Card {
        self.lines.push(Line { text: text.into(), muted });
        self
    }
}

pub struct Bubble {
    pub card: Card,
    pub born_ms: u64,
}

pub struct Press {
    pub start: (i32, i32),
    pub anchor: (i32, i32),
    pub dragging: bool,
}

pub struct Pet {
    pub hwnd: HWND,
    surface: Surface,
    /// Top-left of the creature box, in physical screen pixels.
    pub anchor: (i32, i32),
    pub visible: bool,
    pub bubble: Option<Bubble>,
    pub anim: Option<(AnimKind, u64)>,
    pub hover_btn: Option<usize>,
    pub mouse_inside: bool,
    pub press: Option<Press>,
    hits: Vec<(R, Action)>,
    bubble_rect: Option<R>,
    box_rect: R,
    pub win: IRect,
    pub scale: f32,
    pub frames: u64,
    /// Idle blink: eyes shut for one frame.
    pub eyes_shut: bool,
    /// Total time spent rendering (diagnostics).
    pub render_us: u64,
}

pub struct View<'a> {
    pub pal: &'a Palette,
    pub who: Character,
    /// 0 newly arrived, 1 growing up, 2 fully grown.
    pub stage: u8,
    pub expr: Expr,
    pub attention: Acc,
    pub size: f32,
    pub clock: Option<String>,
    /// Asleep in the burrow for the night.
    pub nestled: bool,
}

impl Pet {
    pub fn new(hwnd: HWND) -> Pet {
        Pet {
            hwnd,
            surface: Surface::empty(),
            anchor: (0, 0),
            visible: true,
            bubble: None,
            anim: None,
            hover_btn: None,
            mouse_inside: false,
            press: None,
            hits: Vec::new(),
            bubble_rect: None,
            box_rect: R::default(),
            win: IRect::default(),
            scale: 1.0,
            frames: 0,
            eyes_shut: false,
            render_us: 0,
        }
    }

    pub fn box_size(scale: f32) -> (i32, i32) {
        ((BOX_W * scale).round() as i32, (BOX_H * scale).round() as i32)
    }

    /// Scale for the monitor under the anchor × user size preference.
    pub fn compute_scale(&self, size: f32) -> f32 {
        let dpi = platform::monitor_at(self.anchor.0 + 20, self.anchor.1 + 20, false).map(|m| m.dpi).unwrap_or(96);
        dpi as f32 / 96.0 * size
    }

    /// Default spot: bottom-right of the primary work area, sitting on the taskbar.
    pub fn default_anchor(size: f32) -> (i32, i32) {
        let m = platform::primary_monitor();
        let s = m.dpi as f32 / 96.0 * size;
        let (bw, bh) = Pet::box_size(s);
        (m.work.r - bw - (28.0 * s) as i32, m.work.b - bh)
    }

    /// Keeps the creature fully on a monitor; snaps onto the taskbar edge when close to it.
    pub fn clamp_anchor(&mut self, size: f32) {
        let s = self.compute_scale(size);
        let (bw, bh) = Pet::box_size(s);
        let cx = self.anchor.0 + bw / 2;
        let cy = self.anchor.1 + bh / 2;
        let m = platform::monitor_at(cx, cy, false).unwrap_or_else(platform::primary_monitor);
        let wa = m.work;
        let mut x = self.anchor.0.clamp(wa.l, (wa.r - bw).max(wa.l));
        let mut y = self.anchor.1.clamp(wa.t, (wa.b - bh).max(wa.t));
        if (wa.b - bh - y).abs() < (18.0 * s) as i32 {
            y = wa.b - bh;
        }
        if (x - wa.l).abs() < (8.0 * s) as i32 {
            x = wa.l;
        }
        self.anchor = (x, y);
    }

    pub fn start_anim(&mut self, kind: AnimKind) {
        self.anim = Some((kind, tick_ms()));
    }

    /// True while something is moving (the frame timer should run).
    pub fn animating(&self) -> bool {
        let now = tick_ms();
        let a = self.anim.is_some_and(|(k, t0)| now.saturating_sub(t0) < k.duration_ms());
        let b = self.bubble.as_ref().is_some_and(|b| now.saturating_sub(b.born_ms) < 160);
        a || b
    }

    pub fn finish_anim_if_done(&mut self) {
        if let Some((k, t0)) = self.anim {
            if tick_ms().saturating_sub(t0) >= k.duration_ms() {
                self.anim = None;
            }
        }
    }

    pub fn pose(&self, expr: Expr, attention: Acc, stage: u8, nestled: bool) -> Pose {
        let mut pose = Pose::base(expr);
        pose.stage = stage;
        // Asleep in the burrow for the night. Applied before the animation, and skipped while the
        // two transitions are running, because `nestled` and those two both *set* the body
        // transform: applying both would put the companion down a well.
        if nestled && !matches!(self.anim.map(|(k, _)| k), Some(AnimKind::TurnIn | AnimKind::GetUp)) {
            creature::nestled(&mut pose);
        }
        if self.eyes_shut {
            pose.blink = 1.0;
        }
        if attention != Acc::None && expr != Expr::Asleep && expr != Expr::Carried {
            pose.acc = attention;
            pose.look_y = -0.4;
            pose.look_x = 0.5;
        }
        if let Some((k, t0)) = self.anim {
            let t = tick_ms().saturating_sub(t0) as f32 / k.duration_ms() as f32;
            if t < 1.0 {
                if attention != Acc::None {
                    pose.acc_t = t;
                }
                creature::animate(k, t, &mut pose);
            }
        }
        pose
    }

    pub fn show_bubble(&mut self, card: Card) {
        self.bubble = Some(Bubble { card, born_ms: tick_ms() });
        self.hover_btn = None;
    }

    pub fn replace_card(&mut self, card: Card) {
        match &mut self.bubble {
            Some(b) => b.card = card,
            None => self.show_bubble(card),
        }
    }

    pub fn bubble_kind(&self) -> Option<BubbleKind> {
        self.bubble.as_ref().map(|b| b.card.kind)
    }

    // -----------------------------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------------------------

    pub fn render(&mut self, v: &View) {
        if !self.visible {
            return;
        }
        let t0 = std::time::Instant::now();
        self.render_inner(v);
        self.render_us += t0.elapsed().as_micros() as u64;
    }

    fn render_inner(&mut self, v: &View) {
        let s = self.compute_scale(v.size);
        self.scale = s;
        let (bw, bh) = Pet::box_size(s);
        let boxr = IRect::xywh(self.anchor.0, self.anchor.1, bw, bh);
        let mon = platform::monitor_at(boxr.l + bw / 2, boxr.t + bh / 2, false).unwrap_or_else(platform::primary_monitor);

        // Bubble placement (screen space)
        let card = self.bubble.as_ref().map(|b| &b.card);
        let mut bubble_screen: Option<(IRect, bool, f32)> = None; // rect, tail_down, tail_x (screen)
        if let Some(card) = card {
            let (cw, ch, _) = layout_card(card, s, None, 0.0, 0.0, v.pal, None);
            let (cw, ch) = (cw.ceil() as i32, ch.ceil() as i32);
            let tail = (9.0 * s) as i32;
            let margin = (8.0 * s) as i32;
            let cx = boxr.l + bw / 2;
            let x = (cx - cw / 2).clamp(mon.work.l + margin, (mon.work.r - cw - margin).max(mon.work.l + margin));
            let head_top = boxr.t + (18.0 * s) as i32;
            let above_y = head_top - tail - ch;
            let (y, down) = if above_y >= mon.work.t + margin { (above_y, true) } else { (boxr.b + tail - (6.0 * s) as i32, false) };
            bubble_screen = Some((IRect::xywh(x, y, cw, ch), down, cx as f32));
        }
        let clock_screen = if bubble_screen.is_none() {
            v.clock.as_ref().map(|c| {
                let fs = 12.0 * s;
                let w = (text::width(&fonts().semibold, fs, c) + 18.0 * s).ceil() as i32;
                let h = (22.0 * s) as i32;
                IRect::xywh(boxr.l + bw / 2 - w / 2, boxr.t - h + (12.0 * s) as i32, w, h)
            })
        } else {
            None
        };

        let pad = (10.0 * s) as i32;
        let mut win = boxr;
        if let Some((r, _, _)) = bubble_screen {
            win = win.union(&IRect { l: r.l - pad, t: r.t - pad, r: r.r + pad, b: r.b + pad + (10.0 * s) as i32 });
        }
        if let Some(r) = clock_screen {
            win = win.union(&r);
        }
        self.win = win;
        if !self.surface.ensure(win.w(), win.h()) {
            return;
        }
        let (ox, oy) = (win.l as f32, win.t as f32);
        let bubble_pop = self.bubble.as_ref().map(|b| ((tick_ms().saturating_sub(b.born_ms)) as f32 / 160.0).min(1.0)).unwrap_or(1.0);
        let pose = self.pose(v.expr, v.attention, v.stage, v.nestled);
        let hover = self.hover_btn;
        let mut hits = Vec::new();
        let mut bubble_rect = None;
        {
            let Some(mut pm) = self.surface.canvas() else { return };

            // Creature
            let base = Transform::from_translate(boxr.l as f32 - ox + ORIGIN_X * s, boxr.t as f32 - oy + ORIGIN_Y * s)
                .pre_scale(UNIT * s, UNIT * s);
            creature::draw(&mut pm, &pose, v.pal.body, v.who, base);

            // Clock pill
            if let (Some(r), Some(c)) = (clock_screen, v.clock.as_ref()) {
                let r = r.offset(-win.l, -win.t);
                fill_rrect(&mut pm, r.l as f32, r.t as f32, r.w() as f32, r.h() as f32, r.h() as f32 / 2.0, color(v.pal.surface, 0.92));
                let fs = 12.0 * s;
                text::draw_centered(&mut pm, &fonts().semibold, fs, (r.l + r.w() / 2) as f32, ui::baseline(r.t as f32, r.h() as f32, fs), v.pal.text, 1.0, c);
            }

            // Bubble
            if let (Some(card), Some((r, down, tail_x))) = (card, bubble_screen) {
                let r = r.offset(-win.l, -win.t);
                let alpha = bubble_pop;
                let lift = (1.0 - bubble_pop) * 6.0 * s;
                let (x, y, w, h) = (r.l as f32, r.t as f32 + if down { lift } else { -lift }, r.w() as f32, r.h() as f32);
                let rad = 16.0 * s;
                soft_shadow(&mut pm, x, y, w, h, rad, s, 0.10 * alpha);
                // Tail
                let tx = (tail_x - ox).clamp(x + rad + 6.0 * s, x + w - rad - 6.0 * s);
                let tail = 9.0 * s;
                let mut pb = PathBuilder::new();
                if down {
                    pb.move_to(tx - 9.0 * s, y + h - 1.0);
                    pb.quad_to(tx - 1.0 * s, y + h + 2.0 * s, tx + 2.0 * s, y + h + tail);
                    pb.quad_to(tx + 3.0 * s, y + h + 2.0 * s, tx + 9.0 * s, y + h - 1.0);
                } else {
                    pb.move_to(tx - 9.0 * s, y + 1.0);
                    pb.quad_to(tx - 1.0 * s, y - 2.0 * s, tx + 2.0 * s, y - tail);
                    pb.quad_to(tx + 3.0 * s, y - 2.0 * s, tx + 9.0 * s, y + 1.0);
                }
                pb.close();
                fill_path(&mut pm, pb.finish(), color(v.pal.surface, alpha), Transform::identity());
                fill_rrect(&mut pm, x, y, w, h, rad, color(v.pal.surface, alpha));
                stroke_rrect(&mut pm, x + 0.5, y + 0.5, w - 1.0, h - 1.0, rad, color(if v.pal.dark { 0xFFFFFF } else { 0x000000 }, 0.07 * alpha), 1.0);
                if card.important {
                    fill_rrect(&mut pm, x + 6.0 * s, y + 14.0 * s, 3.5 * s, h - 28.0 * s, 2.0 * s, color(0xFFB020, alpha));
                }
                let (_, _, h2) = layout_card(card, s, Some(&mut pm), x, y, v.pal, hover);
                hits = h2;
                bubble_rect = Some(R::new(x, y - if down { 0.0 } else { tail }, w, h + tail));
            }
        }
        self.hits = hits;
        self.bubble_rect = bubble_rect;
        self.box_rect = R::new(boxr.l as f32 - ox, boxr.t as f32 - oy, bw as f32, bh as f32);
        self.surface.present_layered(self.hwnd, win.l, win.t);
        self.frames += 1;
    }

    /// Releases the pixel buffer (e.g. while hidden). Recreated on the next render.
    pub fn release_surface(&mut self) {
        self.surface.release();
    }

    #[cfg(feature = "devtools")]
    pub fn snapshot_rgba(&mut self) -> (Vec<u8>, u32, u32) {
        (self.surface.rgba_copy(), self.surface.w as u32, self.surface.h as u32)
    }

    // -----------------------------------------------------------------------------------------
    // Hit testing (client coordinates)
    // -----------------------------------------------------------------------------------------

    pub fn hit_button(&self, x: i32, y: i32) -> Option<(usize, Action)> {
        self.hits.iter().enumerate().find(|(_, (r, _))| r.contains(x as f32, y as f32)).map(|(i, (_, a))| (i, *a))
    }

    pub fn in_bubble(&self, x: i32, y: i32) -> bool {
        self.bubble_rect.is_some_and(|r| r.contains(x as f32, y as f32))
    }

    pub fn in_creature(&self, x: i32, y: i32) -> bool {
        self.box_rect.contains(x as f32, y as f32)
    }
}

/// Lays out (and, with `pm`, draws) a card at (x, y). Returns (width, height, button hits).
pub fn layout_card(
    card: &Card,
    s: f32,
    mut pm: Option<&mut PixmapMut>,
    x: f32,
    y: f32,
    pal: &Palette,
    hover: Option<usize>,
) -> (f32, f32, Vec<(R, Action)>) {
    let f = fonts();
    let pad = if card.kind == BubbleKind::Tooltip { 11.0 * s } else { 15.0 * s };
    let inner_max = MAX_CARD_W * s - pad * 2.0;
    let mut hits = Vec::new();

    // Measure natural width
    let big_size = 30.0 * s;
    let title_size = 14.0 * s;
    let body_size = 13.0 * s;
    let mut natural: f32 = 0.0;
    if let Some(b) = &card.big {
        natural = natural.max(text::width(&f.semibold, big_size, b));
    }
    if let Some(t) = &card.title {
        natural = natural.max(text::width(&f.semibold, title_size, t));
    }
    for l in &card.lines {
        natural = natural.max(text::width(&f.regular, body_size, &l.text));
    }
    let btn_h = 30.0 * s;
    let gap = 6.0 * s;
    let btn_widths: Vec<f32> = card.buttons.iter().map(|(l, _, _)| ui::button_width(l, s)).collect();
    let btn_total: f32 = btn_widths.iter().sum::<f32>() + gap * (btn_widths.len().saturating_sub(1)) as f32;
    natural = natural.max(btn_total);
    if card.water.is_some() {
        natural = natural.max(190.0 * s);
    }
    let inner = natural.min(inner_max).max(if card.kind == BubbleKind::Tooltip { 0.0 } else { 150.0 * s });
    let w = inner + pad * 2.0;

    let mut cy = y + pad;
    let lx = x + pad;
    if let Some(b) = &card.big {
        let asc = text::ascent(&f.semibold, big_size);
        if let Some(pm) = pm.as_deref_mut() {
            text::draw(pm, &f.semibold, big_size, lx, cy + asc * 0.95, pal.text, 1.0, b);
        }
        cy += asc * 1.12;
    }
    if let Some(t) = &card.title {
        let asc = text::ascent(&f.semibold, title_size);
        for (i, l) in text::wrap(&f.semibold, title_size, t, inner).iter().enumerate() {
            if let Some(pm) = pm.as_deref_mut() {
                text::draw(pm, &f.semibold, title_size, lx, cy + asc, pal.text, 1.0, l);
            }
            cy += title_size * 1.35;
            let _ = i;
        }
        cy += 2.0 * s;
    }
    for line in &card.lines {
        let asc = text::ascent(&f.regular, body_size);
        let color_ = if line.muted { pal.muted } else { pal.text };
        for l in text::wrap(&f.regular, body_size, &line.text, inner) {
            if let Some(pm) = pm.as_deref_mut() {
                text::draw(pm, &f.regular, body_size, lx, cy + asc, color_, 1.0, &l);
            }
            cy += body_size * 1.42;
        }
    }
    if let Some((count, goal)) = card.water {
        cy += 4.0 * s;
        let n = goal.min(12);
        let dsz = 15.0 * s;
        let step = (inner / n as f32).min(19.0 * s);
        if let Some(pm) = pm.as_deref_mut() {
            for i in 0..n {
                let filled = i < count;
                let t = Transform::from_translate(lx + step * i as f32 + dsz * 0.4, cy + dsz * 0.5).pre_scale(dsz / 19.0, dsz / 19.0);
                if filled {
                    creature::draw_drop(pm, t, 0x4DB3F2, 1.0);
                } else {
                    // Empty glass: a soft outline so it reads on both themes.
                    creature::draw_drop_outline(pm, t, if pal.dark { 0x5A6273 } else { 0xC3CAD6 });
                }
            }
        }
        cy += dsz + 8.0 * s;
    }
    if !card.buttons.is_empty() {
        cy += 6.0 * s;
        let mut bx = lx;
        for (i, ((label, action, style), bw)) in card.buttons.iter().zip(&btn_widths).enumerate() {
            if bx + bw > lx + inner + 0.5 && bx > lx {
                bx = lx;
                cy += btn_h + gap;
            }
            let r = R::new(bx, cy, *bw, btn_h);
            if let Some(pm) = pm.as_deref_mut() {
                ui::button(pm, r, label, *style, pal, s, hover == Some(i));
            }
            hits.push((r, *action));
            bx += bw + gap;
        }
        cy += btn_h;
    } else {
        cy -= 3.0 * s;
    }
    let h = cy - y + pad;
    (w, h, hits)
}
