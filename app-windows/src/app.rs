//! The application controller: owns settings/state, turns core decisions into what the creature
//! does, and keeps exactly the timers that are needed — nothing ticks unless something is due.

use crate::pet::{Action, BubbleKind, Card, Pet, View};
use crate::platform::{self, now_unix, tick_ms};
use crate::render::creature::{Acc, AnimKind, Expr};
use crate::render::ui::BtnStyle;
use crate::render::Palette;
use crate::settings_ui::SettingsUi;
use crate::storage::Storage;
use crate::tray::Tray;
use chrono::{Local, Timelike};
use pip_core::schedule::{self, Delivery, Due, ItemKey, Priority};
use pip_core::settings::Liveliness;
use pip_core::time::{local, long_date, TimeOfDay};
use pip_core::{messages, mood, Mood, Settings, State};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const WM_TRAY: u32 = WM_APP + 1;
pub const WM_APPBAR: u32 = WM_APP + 2;

pub const T_SCHED: usize = 1;
pub const T_ANIM: usize = 2;
pub const T_IDLE: usize = 3;
pub const T_BUBBLE: usize = 4;
pub const T_CLOCK: usize = 5;
pub const T_AWAY: usize = 6;
pub const T_SAVE: usize = 7;
pub const T_NEXT: usize = 8;
pub const T_BLINK: usize = 9;
pub const T_TRIM: usize = 10;

const FRAME_MS: u32 = 33;
/// Non-important reminders wait this long after launch (let the desktop settle at login).
const STARTUP_GRACE_MS: u64 = 45_000;
const AWAY_AFTER_MS: u64 = 5 * 60_000;

pub enum Effect {
    Menu(i32, i32),
    OpenSettings,
    CloseSettings,
    Quit,
}

pub struct Pending {
    pub key: ItemKey,
    pub occurrence: i64,
    pub priority: Priority,
    pub title: String,
    pub body: String,
    pub shown: bool,
}

pub struct App {
    pub hwnd: HWND,
    pub settings: Settings,
    pub state: State,
    pub store: Storage,
    pub pet: Pet,
    pub tray: Tray,
    pub pending: Vec<Pending>,
    pub ui: Option<SettingsUi>,
    pub effects: Vec<Effect>,
    pub started_ms: u64,
    pub startup_ms: u64,
    pub locked: bool,
    pub fullscreen: bool,
    pub away: bool,
    pub night: bool,
    wake_until_ms: u64,
    last_micro_ms: u64,
    last_mood: Option<Mood>,
    settings_dirty: bool,
    state_dirty: bool,
    chime: Option<platform::Chime>,
    rng: u64,
    anim_timer: bool,
    first_run: bool,
    pub dev_mode: bool,
}

pub fn hm(ts: i64) -> String {
    let l = local(&Local, ts);
    format!("{:02}:{:02}", l.hour(), l.minute())
}

impl App {
    pub fn new(hwnd: HWND, store: Storage, settings: Settings, state: State, first_run: bool) -> App {
        App {
            hwnd,
            settings,
            state,
            store,
            pet: Pet::new(hwnd),
            tray: Tray::new(hwnd, WM_TRAY),
            pending: Vec::new(),
            ui: None,
            effects: Vec::new(),
            started_ms: tick_ms(),
            startup_ms: 0,
            locked: false,
            fullscreen: false,
            away: false,
            night: false,
            wake_until_ms: 0,
            last_micro_ms: tick_ms(),
            last_mood: None,
            settings_dirty: false,
            state_dirty: false,
            chime: None,
            rng: tick_ms() ^ 0x9E37_79B9_7F4A_7C15,
            anim_timer: false,
            first_run,
            dev_mode: Storage::is_custom_dir(),
        }
    }

    pub fn start(&mut self) {
        let now = now_unix();
        let today = local(&Local, now).date();
        if self.state.roll_day(&self.settings, today) {
            self.state.water.seen_since = Some(now);
        }
        self.pet.anchor = match self.state.desktop_pos {
            Some([x, y]) if platform::monitor_at(x + 20, y + 20, true).is_some() => (x, y),
            _ => Pet::default_anchor(self.size()),
        };
        self.pet.clamp_anchor(self.size());
        self.pet.visible = !self.state.pet_hidden;
        self.night = schedule::is_quiet(&self.settings, now, &Local);
        platform::allow_dark_menus(self.palette().dark);
        self.apply_topmost();
        if self.pet.visible {
            unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
            self.pet.start_anim(AnimKind::PopIn);
        }
        self.render();
        self.startup_ms = platform::ms_since_process_start();
        let tip = self.tray_tip();
        self.tray.add(self.settings.pet.color.rgb(), false, &tip);
        platform::register_appbar(self.hwnd, WM_APPBAR);
        unsafe {
            windows_sys::Win32::System::RemoteDesktop::WTSRegisterSessionNotification(self.hwnd, 0 /* NOTIFY_FOR_THIS_SESSION */);
        }
        if !self.dev_mode {
            platform::set_autostart(self.settings.pet.start_with_system);
        }
        if self.first_run {
            let (t, b) = messages::first_run(&self.settings.pet.name);
            let mut card = Card::new(BubbleKind::Message).line(b, false);
            card.title = Some(t);
            card.buttons.push(("Nice to meet you".into(), Action::Close, BtnStyle::Primary));
            card.buttons.push(("Settings".into(), Action::Settings, BtnStyle::Secondary));
            self.pet.show_bubble(card);
            self.set_timer(T_BUBBLE, 60_000);
            self.anim(AnimKind::Wave);
            self.settings_dirty = true;
            self.state_dirty = true;
            self.schedule_save();
        }
        self.store.log(&format!("started in {} ms (v{})", self.startup_ms, env!("CARGO_PKG_VERSION")));
        // Once startup settles, hand back pages that were only needed to get going.
        self.set_timer(T_TRIM, 30_000);
        self.reschedule();
        self.arm_idle();
        self.arm_clock();
    }

    // -----------------------------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------------------------

    pub fn size(&self) -> f32 {
        self.settings.pet.size.factor()
    }

    pub fn palette(&self) -> Palette {
        Palette::new(self.settings.pet.theme, self.settings.pet.color)
    }

    fn rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn rand_range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.rand() % (hi - lo).max(1)
    }

    pub fn set_timer(&self, id: usize, ms: u32) {
        unsafe { SetTimer(self.hwnd, id, ms.max(10), None) };
    }

    pub fn kill_timer(&self, id: usize) {
        unsafe { KillTimer(self.hwnd, id) };
    }

    fn asleep(&self) -> bool {
        self.locked || self.away || (self.night && tick_ms() > self.wake_until_ms)
    }

    pub fn expr(&self) -> Expr {
        if self.pet.press.as_ref().is_some_and(|p| p.dragging) {
            return Expr::Carried;
        }
        if self.asleep() {
            return Expr::Asleep;
        }
        match mood(&self.settings, &self.state, now_unix(), &Local) {
            Mood::Content => Expr::Content,
            Mood::Happy => Expr::Happy,
            Mood::Proud => Expr::Proud,
            Mood::Thirsty => Expr::Thirsty,
            Mood::Sleepy => Expr::Content, // woken up during quiet hours
        }
    }

    /// Badge shown on the pet while something waits and its bubble is folded away.
    fn attention(&self) -> Acc {
        let showing = self.pet.bubble_kind() == Some(BubbleKind::Notify);
        match self.pending.first() {
            Some(p) if !showing => match p.key {
                ItemKey::Water => Acc::Drop,
                ItemKey::Custom(_) => Acc::Alert,
            },
            _ => Acc::None,
        }
    }

    fn water_today(&self) -> u32 {
        let today = local(&Local, now_unix()).date();
        if self.state.water.day == Some(today) {
            self.state.water.count
        } else {
            0
        }
    }

    fn tray_tip(&self) -> String {
        let name = &self.settings.pet.name;
        let mut tip = if self.settings.water.enabled {
            format!("{name} · {} of {} glasses today", self.water_today(), self.settings.water.daily_goal)
        } else {
            name.clone()
        };
        if let Some(t) = self.state.dnd_until.filter(|&t| t > now_unix()) {
            tip.push_str(&format!("\nDo not disturb until {}", hm(t)));
        }
        tip
    }

    // -----------------------------------------------------------------------------------------
    // Rendering & animation clock
    // -----------------------------------------------------------------------------------------

    pub fn render(&mut self) {
        let pal = self.palette();
        let clock = self.settings.pet.show_clock.then(|| hm(now_unix()));
        let view = View { pal: &pal, expr: self.expr(), attention: self.attention(), size: self.size(), clock };
        self.pet.render(&view);
        let want = self.pet.visible && self.pet.animating();
        if want && !self.anim_timer {
            self.set_timer(T_ANIM, FRAME_MS);
            self.anim_timer = true;
        } else if !want && self.anim_timer {
            self.kill_timer(T_ANIM);
            self.anim_timer = false;
        }
    }

    pub fn anim(&mut self, kind: AnimKind) {
        if !self.pet.visible {
            return;
        }
        self.pet.start_anim(kind);
        self.render();
    }

    fn on_frame(&mut self) {
        self.pet.finish_anim_if_done();
        self.render();
    }

    /// Idle life: blinks and the occasional small animation. Nothing at all while asleep/hidden.
    pub fn arm_idle(&mut self) {
        if !self.pet.visible || self.asleep() || self.fullscreen {
            self.kill_timer(T_IDLE);
            return;
        }
        let (lo, hi) = match self.settings.pet.liveliness {
            Liveliness::Calm => (5_000, 11_000),
            Liveliness::Normal => (4_500, 9_000),
            Liveliness::Lively => (2_200, 5_500),
        };
        let d = self.rand_range(lo, hi) as u32;
        self.set_timer(T_IDLE, d);
    }

    fn on_idle(&mut self) {
        self.kill_timer(T_IDLE);
        // PIP_MEASURE_AWAKE: measurement-only switch so tools/measure.ps1 can record the worst case
        // (pet awake and blinking) even when nobody is touching the machine.
        if platform::idle_ms() >= AWAY_AFTER_MS && std::env::var_os("PIP_MEASURE_AWAKE").is_none() {
            self.set_away(true);
            return;
        }
        let now = tick_ms();
        let (lo, hi) = match self.settings.pet.liveliness {
            Liveliness::Calm => (10 * 60_000, 20 * 60_000),
            Liveliness::Normal => (3 * 60_000, 6 * 60_000),
            Liveliness::Lively => (60_000, 3 * 60_000),
        };
        let due_micro = now.saturating_sub(self.last_micro_ms) >= self.rand_range(lo, hi);
        let kind = if due_micro && self.pet.bubble.is_none() {
            self.last_micro_ms = now;
            let options: &[AnimKind] = match self.settings.pet.liveliness {
                Liveliness::Calm => &[AnimKind::LookAround],
                Liveliness::Normal => &[AnimKind::LookAround, AnimKind::Stretch, AnimKind::Wiggle],
                Liveliness::Lively => &[AnimKind::LookAround, AnimKind::Stretch, AnimKind::Wiggle, AnimKind::Hop],
            };
            options[(self.rand() % options.len() as u64) as usize]
        } else {
            AnimKind::Blink
        };
        if self.pet.anim.is_none() {
            if kind == AnimKind::Blink {
                // A blink is two frames (shut, then open 110 ms later) instead of running the
                // 30 fps animation clock — the single biggest idle-CPU saving.
                self.pet.eyes_shut = true;
                self.render();
                self.set_timer(T_BLINK, 110);
            } else {
                self.anim(kind);
            }
        }
        self.arm_idle();
    }

    fn set_away(&mut self, away: bool) {
        if self.away == away {
            return;
        }
        self.away = away;
        if away {
            self.kill_timer(T_IDLE);
            self.set_timer(T_AWAY, 15_000); // the one poll we allow: "are you back?"
            self.render();
        } else {
            self.kill_timer(T_AWAY);
            if !self.asleep() {
                self.anim(AnimKind::WakeUp);
            }
            self.reschedule();
            self.arm_idle();
        }
    }

    /// Minute-aligned clock tick, only when a clock is actually visible.
    pub fn arm_clock(&self) {
        let visible = self.pet.visible
            && (self.settings.pet.show_clock || self.pet.bubble_kind() == Some(BubbleKind::Info));
        if !visible {
            self.kill_timer(T_CLOCK);
            return;
        }
        let now = chrono::Local::now();
        let ms_into_minute = now.second() as u64 * 1000 + now.timestamp_subsec_millis() as u64;
        self.set_timer(T_CLOCK, (60_000 - ms_into_minute + 50) as u32);
    }

    // -----------------------------------------------------------------------------------------
    // Timers
    // -----------------------------------------------------------------------------------------

    pub fn on_timer(&mut self, id: usize) {
        match id {
            T_ANIM => self.on_frame(),
            T_IDLE => self.on_idle(),
            T_SCHED => {
                self.kill_timer(T_SCHED);
                self.reschedule();
            }
            T_BUBBLE => {
                self.kill_timer(T_BUBBLE);
                self.bubble_timeout();
            }
            T_CLOCK => {
                if self.pet.bubble_kind() == Some(BubbleKind::Info) {
                    let c = self.info_card();
                    self.pet.replace_card(c);
                }
                self.render();
                self.arm_clock();
            }
            T_AWAY => {
                if platform::idle_ms() < 15_000 {
                    self.set_away(false);
                }
            }
            T_SAVE => {
                self.kill_timer(T_SAVE);
                self.flush_save();
            }
            T_BLINK => {
                self.kill_timer(T_BLINK);
                self.pet.eyes_shut = false;
                self.render();
            }
            T_TRIM => {
                self.kill_timer(T_TRIM);
                platform::trim_working_set();
            }
            T_NEXT => {
                self.kill_timer(T_NEXT);
                if self.pet.bubble.is_none() {
                    self.show_front_pending(false);
                }
            }
            _ => {}
        }
    }

    fn bubble_timeout(&mut self) {
        let Some(kind) = self.pet.bubble_kind() else { return };
        if matches!(kind, BubbleKind::Info | BubbleKind::Tooltip) && self.pet.mouse_inside {
            self.set_timer(T_BUBBLE, 4_000); // don't pull it away while it's being read
            return;
        }
        self.pet.bubble = None; // a Notify bubble folds into the badge; the item stays pending
        self.render();
        self.arm_clock();
        if kind == BubbleKind::Message && self.pending.iter().any(|p| !p.shown) {
            self.set_timer(T_NEXT, 600);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Scheduling
    // -----------------------------------------------------------------------------------------

    /// Everything waits while the session is locked, or while Windows reports a fullscreen game,
    /// video or presentation (asked at delivery time, so a missed app-bar event can't leak a popup).
    fn holding_all(&self) -> bool {
        self.locked || (self.settings.notify.pause_in_fullscreen && platform::user_busy_fullscreen())
    }

    pub fn reschedule(&mut self) {
        let now = now_unix();
        let tz = Local;
        let today = local(&tz, now).date();
        if self.state.roll_day(&self.settings, today) {
            self.state.water.seen_since = Some(now);
            self.state_dirty = true;
            self.schedule_save();
        }
        if self.state.dnd_until.is_some_and(|t| t <= now) {
            self.state.dnd_until = None;
            self.state_dirty = true;
            self.schedule_save();
        }

        // Night (quiet hours) naps.
        let night = schedule::is_quiet(&self.settings, now, &tz);
        if night != self.night {
            self.night = night;
            if !night {
                self.anim(AnimKind::WakeUp);
            }
            self.arm_idle();
        }

        let due = schedule::due_items(&self.settings, &self.state, now, &tz);
        // Drop pending items that are no longer due (settings changed, handled elsewhere).
        let before = self.pending.len();
        self.pending.retain(|p| due.iter().any(|d| d.key == p.key));
        if self.pending.len() != before && self.pet.bubble_kind() == Some(BubbleKind::Notify) && self.pending.is_empty() {
            self.pet.bubble = None;
        }

        let in_grace = tick_ms().saturating_sub(self.started_ms) < STARTUP_GRACE_MS;
        let held_busy = !due.is_empty() && !self.locked && self.holding_all();
        if !self.holding_all() {
            for d in due {
                if self.pending.iter().any(|p| p.key == d.key) {
                    continue;
                }
                if d.key == ItemKey::Water && self.away {
                    continue; // they can't drink while away; ask when they're back
                }
                if in_grace && d.priority < Priority::High {
                    continue;
                }
                if let Delivery::Now { sound } = schedule::delivery(&self.settings, &self.state, d.priority, now, &tz) {
                    self.present(d, sound);
                }
            }
        }

        let mut next = schedule::next_event(&self.settings, &self.state, now, &tz);
        if held_busy {
            next = next.min(now + 60); // fullscreen app: look again in a minute
        }
        if in_grace {
            next = next.min(now + ((STARTUP_GRACE_MS - tick_ms().saturating_sub(self.started_ms)) / 1000) as i64 + 1);
        }
        let ms = ((next - now).max(0) * 1000).clamp(500, 3_600_000) as u32;
        self.set_timer(T_SCHED, ms);

        let m = mood(&self.settings, &self.state, now, &tz);
        if self.last_mood != Some(m) {
            self.last_mood = Some(m);
        }
        self.render();
        self.update_tray();
    }

    fn update_tray(&mut self) {
        let tip = self.tray_tip();
        let attention = !self.pending.is_empty();
        self.tray.update(self.settings.pet.color.rgb(), attention, &tip);
    }

    fn present(&mut self, d: Due, sound: bool) {
        let name = self.settings.pet.name.clone();
        let seq = self.state.next_msg();
        let (title, body) = match &d.key {
            ItemKey::Water => {
                if d.late {
                    ("Water".to_string(), messages::water_late(&name))
                } else {
                    messages::water_nudge(&name, seq, self.water_today(), self.settings.water.daily_goal)
                }
            }
            ItemKey::Custom(id) => {
                let r = self.settings.reminders.iter().find(|r| &r.id == id);
                let title = r.map(|r| r.title.clone()).unwrap_or_else(|| "Reminder".into());
                (title, messages::reminder_intro(&name, d.late, &hm(d.occurrence)))
            }
        };
        self.pending.push(Pending { key: d.key, occurrence: d.occurrence, priority: d.priority, title, body, shown: false });
        let showing_notify = self.pet.bubble_kind() == Some(BubbleKind::Notify);
        if !showing_notify {
            self.show_front_pending(sound && d.priority >= Priority::Normal);
        } else if sound {
            self.play_chime();
        }
        self.update_tray();
    }

    fn notify_card(&self, p: &Pending) -> Card {
        let mut card = Card::new(BubbleKind::Notify).line(p.body.clone(), p.key != ItemKey::Water);
        card.title = Some(p.title.clone());
        card.important = p.priority == Priority::High;
        let snooze = format!("Snooze {}m", self.settings.notify.snooze_min);
        match p.key {
            ItemKey::Water => {
                card.water = Some((self.water_today(), self.settings.water.daily_goal));
                card.buttons.push(("Drank it".into(), Action::Done, BtnStyle::Primary));
                card.buttons.push((snooze, Action::Snooze, BtnStyle::Secondary));
                card.buttons.push(("Not now".into(), Action::Dismiss, BtnStyle::Ghost));
            }
            ItemKey::Custom(_) => {
                card.buttons.push(("Done".into(), Action::Done, BtnStyle::Primary));
                card.buttons.push((snooze, Action::Snooze, BtnStyle::Secondary));
                card.buttons.push(("Dismiss".into(), Action::Dismiss, BtnStyle::Ghost));
            }
        }
        card
    }

    /// Shows the first pending item: as a bubble if the pet is out, otherwise as a Windows toast.
    fn show_front_pending(&mut self, sound: bool) {
        let Some(p) = self.pending.first() else { return };
        let card = self.notify_card(p);
        let (title, body, prio) = (p.title.clone(), p.body.clone(), p.priority);
        self.pending[0].shown = true;
        if self.pet.visible {
            self.pet.show_bubble(card);
            self.set_timer(T_BUBBLE, self.settings.notify.bubble_minutes * 60_000);
            if self.asleep() {
                self.wake_until_ms = tick_ms() + 60_000;
            }
            self.anim(AnimKind::Wave);
            self.arm_clock();
        } else if prio >= Priority::Normal {
            self.tray.balloon(&title, &body);
        }
        if sound {
            self.play_chime();
        }
    }

    fn play_chime(&mut self) {
        if !self.settings.notify.sound {
            return;
        }
        self.chime.get_or_insert_with(platform::Chime::new).play();
    }

    fn message(&mut self, text: String, secs: u32) {
        if !self.pet.visible {
            return;
        }
        self.pet.show_bubble(Card::new(BubbleKind::Message).line(text, false));
        self.set_timer(T_BUBBLE, secs * 1000);
        self.arm_clock();
    }

    // -----------------------------------------------------------------------------------------
    // User actions
    // -----------------------------------------------------------------------------------------

    pub fn act(&mut self, action: Action) {
        let now = now_unix();
        match action {
            Action::Close => {
                self.pet.bubble = None;
                self.kill_timer(T_BUBBLE);
                if self.pending.iter().any(|p| !p.shown) {
                    self.set_timer(T_NEXT, 400);
                }
            }
            Action::Settings => {
                self.pet.bubble = None;
                self.effects.push(Effect::OpenSettings);
            }
            Action::Drink => self.drink(),
            Action::Done | Action::Snooze | Action::Dismiss => {
                let Some(p) = (!self.pending.is_empty()).then(|| self.pending.remove(0)) else {
                    self.pet.bubble = None;
                    self.render();
                    return;
                };
                match (&p.key, action) {
                    (ItemKey::Water, Action::Done) => {
                        self.pending.insert(0, p);
                        self.drink();
                        return;
                    }
                    (ItemKey::Water, Action::Snooze) => {
                        schedule::snooze_water(&mut self.state, now + self.settings.notify.snooze_min as i64 * 60);
                    }
                    (ItemKey::Water, _) => schedule::dismiss_water(&mut self.state, now),
                    (ItemKey::Custom(id), Action::Snooze) => {
                        schedule::snooze_custom(&mut self.state, id, now + self.settings.notify.snooze_min as i64 * 60);
                    }
                    (ItemKey::Custom(id), _) => {
                        schedule::ack_custom(&mut self.settings, &mut self.state, id, p.occurrence, now);
                        self.settings_dirty = true;
                    }
                }
                self.state_dirty = true;
                self.schedule_save();
                match action {
                    Action::Snooze => {
                        self.message(format!("Okay! I'll check back in {} minutes.", self.settings.notify.snooze_min), 3);
                        self.anim(AnimKind::Nod);
                    }
                    Action::Done => {
                        self.state.bump_affection(3, now);
                        let seq = self.state.next_msg();
                        self.message(messages::done_cheer(seq).to_string(), 3);
                        self.anim(AnimKind::Celebrate);
                    }
                    _ => {
                        self.pet.bubble = None;
                        self.kill_timer(T_BUBBLE);
                        if !self.pending.is_empty() {
                            self.set_timer(T_NEXT, 500);
                        }
                    }
                }
            }
        }
        self.reschedule();
        self.arm_clock();
    }

    pub fn drink(&mut self) {
        let now = now_unix();
        let today = local(&Local, now).date();
        let count = self.state.log_drink(&self.settings, now, today);
        self.pending.retain(|p| p.key != ItemKey::Water);
        self.state_dirty = true;
        self.schedule_save();
        let seq = self.state.next_msg();
        let goal = self.settings.water.daily_goal;
        let mut card = Card::new(BubbleKind::Message).line(messages::water_logged(count, goal, seq), false);
        card.water = Some((count, goal));
        if self.pet.visible {
            self.pet.show_bubble(card);
            self.set_timer(T_BUBBLE, 3_500);
        }
        self.wake_until_ms = tick_ms() + 20_000;
        self.anim(if count == goal { AnimKind::Celebrate } else { AnimKind::Drink });
        self.reschedule();
    }

    pub fn undo_drink(&mut self) {
        self.state.undo_drink();
        self.state_dirty = true;
        self.schedule_save();
        self.message(format!("Okay, un-counted. {} of {} today.", self.water_today(), self.settings.water.daily_goal), 3);
        self.reschedule();
    }

    fn info_card(&self) -> Card {
        let now = now_unix();
        let l = local(&Local, now);
        let mut card = Card::new(BubbleKind::Info);
        card.big = Some(hm(now));
        card = card.line(long_date(l.date()), true);
        let s = &self.settings;
        if s.water.enabled {
            card.water = Some((self.water_today(), s.water.daily_goal));
        }
        if let Some(t) = self.state.dnd_until.filter(|&t| t > now) {
            card = card.line(format!("Do not disturb until {}", hm(t)), false);
        } else if schedule::is_quiet(s, now, &Local) {
            card = card.line(format!("Quiet hours until {}", s.quiet.end), false);
        } else if s.water.enabled {
            match schedule::water_next(s, &self.state, now, &Local) {
                Some(t) if t <= now => card = card.line("Water is due now", false),
                Some(t) if local(&Local, t).date() == l.date() => card = card.line(format!("Next water around {}", hm(t)), false),
                Some(_) => card = card.line("Water reminders resume tomorrow", false),
                None => {}
            }
        }
        // Soonest custom reminder in the next 24 h
        let soon = s
            .reminders
            .iter()
            .filter_map(|r| schedule::reminder_next(r, &self.state, now, &Local).map(|t| (t, r)))
            .filter(|(t, _)| *t - now < 86_400)
            .min_by_key(|(t, _)| *t);
        if let Some((t, r)) = soon {
            let when = if local(&Local, t).date() == l.date() { hm(t) } else { format!("tomorrow {}", hm(t)) };
            card = card.line(format!("{} at {}", r.title, when), false);
        }
        if self.state.streak >= 2 {
            card = card.line(format!("{}-day water streak", self.state.streak), true);
        }
        if s.water.enabled {
            card.buttons.push(("+ Glass of water".into(), Action::Drink, BtnStyle::Primary));
        }
        card.buttons.push(("Settings".into(), Action::Settings, BtnStyle::Secondary));
        card
    }

    fn tooltip_card(&self) -> Card {
        let now = now_unix();
        let l = local(&Local, now);
        let mut c = Card::new(BubbleKind::Tooltip);
        c.title = Some(format!("{}  ·  {}", hm(now), pip_core::time::weekday_name(chrono::Datelike::weekday(&l.date()))));
        c
    }

    // -----------------------------------------------------------------------------------------
    // Mouse on the pet
    // -----------------------------------------------------------------------------------------

    pub fn mouse_down(&mut self, x: i32, y: i32) {
        if self.pet.in_bubble(x, y) {
            return; // handled on button up
        }
        if self.pet.in_creature(x, y) {
            let (sx, sy) = platform::cursor_pos();
            self.pet.press = Some(crate::pet::Press { start: (sx, sy), anchor: self.pet.anchor, dragging: false });
            unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::SetCapture(self.hwnd) };
        }
    }

    pub fn mouse_move(&mut self, x: i32, y: i32) {
        if let Some(p) = &mut self.pet.press {
            let (sx, sy) = platform::cursor_pos();
            let (dx, dy) = (sx - p.start.0, sy - p.start.1);
            if !p.dragging && dx * dx + dy * dy > 25 {
                p.dragging = true;
                self.pet.bubble = None;
            }
            if p.dragging {
                self.pet.anchor = (p.anchor.0 + dx, p.anchor.1 + dy);
                self.render();
            }
            return;
        }
        let hover = self.pet.hit_button(x, y).map(|(i, _)| i);
        if hover != self.pet.hover_btn {
            self.pet.hover_btn = hover;
            self.render();
        }
    }

    pub fn mouse_up(&mut self, x: i32, y: i32) {
        if let Some(p) = self.pet.press.take() {
            unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture() };
            if p.dragging {
                self.pet.clamp_anchor(self.size());
                self.state.desktop_pos = Some([self.pet.anchor.0, self.pet.anchor.1]);
                self.state_dirty = true;
                self.schedule_save();
                self.anim(AnimKind::Wiggle);
                return;
            }
            self.click_creature();
            return;
        }
        if let Some((_, action)) = self.pet.hit_button(x, y) {
            self.act(action);
        } else if self.pet.in_bubble(x, y) {
            if matches!(self.pet.bubble_kind(), Some(BubbleKind::Info | BubbleKind::Message | BubbleKind::Tooltip)) {
                self.act(Action::Close);
            }
        }
    }

    pub fn click_creature(&mut self) {
        let now = now_unix();
        self.state.bump_affection(1, now);
        self.state_dirty = true;
        self.schedule_save();
        let was_asleep = self.asleep();
        if was_asleep {
            self.wake_until_ms = tick_ms() + 20_000;
        }
        if !self.pending.is_empty() && self.pet.bubble_kind() != Some(BubbleKind::Notify) {
            let card = self.notify_card(&self.pending[0]);
            self.pet.show_bubble(card);
            self.set_timer(T_BUBBLE, self.settings.notify.bubble_minutes * 60_000);
        } else if self.pet.bubble_kind() == Some(BubbleKind::Info) {
            self.pet.bubble = None;
        } else if self.pet.bubble_kind() != Some(BubbleKind::Notify) {
            let c = self.info_card();
            self.pet.show_bubble(c);
            self.set_timer(T_BUBBLE, 12_000);
        }
        self.anim(if was_asleep { AnimKind::WakeUp } else { AnimKind::Hop });
        self.arm_clock();
    }

    pub fn double_click(&mut self, x: i32, y: i32) {
        if self.pet.in_creature(x, y) && !self.pet.in_bubble(x, y) && self.settings.water.enabled {
            self.drink();
        }
    }

    pub fn mouse_hover(&mut self, x: i32, y: i32) {
        if self.pet.bubble.is_none() && self.pet.in_creature(x, y) && self.pet.press.is_none() {
            let c = self.tooltip_card();
            self.pet.show_bubble(c);
            self.set_timer(T_BUBBLE, 8_000);
            self.render();
        }
    }

    pub fn mouse_leave(&mut self) {
        self.pet.mouse_inside = false;
        if self.pet.hover_btn.take().is_some() {
            self.render();
        }
        if self.pet.bubble_kind() == Some(BubbleKind::Tooltip) {
            self.pet.bubble = None;
            self.render();
        }
    }

    // -----------------------------------------------------------------------------------------
    // Menu
    // -----------------------------------------------------------------------------------------

    pub const ID_TOGGLE: u32 = 100;
    pub const ID_DRINK: u32 = 101;
    pub const ID_UNDO: u32 = 102;
    pub const ID_DND_1H: u32 = 110;
    pub const ID_DND_MORNING: u32 = 111;
    pub const ID_DND_OFF: u32 = 112;
    pub const ID_TOP: u32 = 120;
    pub const ID_SETTINGS: u32 = 130;
    pub const ID_QUIT: u32 = 140;

    /// Builds the context menu (shared by pet right-click and the tray icon).
    pub fn build_menu(&self) -> HMENU {
        let w = platform::wide;
        unsafe {
            let m = CreatePopupMenu();
            let name = &self.settings.pet.name;
            let toggle = if self.pet.visible { format!("Hide {name}") } else { format!("Show {name}") };
            AppendMenuW(m, MF_STRING, Self::ID_TOGGLE as usize, w(&toggle).as_ptr());
            AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
            if self.settings.water.enabled {
                let label = format!("I drank a glass of water\t{}/{}", self.water_today(), self.settings.water.daily_goal);
                AppendMenuW(m, MF_STRING, Self::ID_DRINK as usize, w(&label).as_ptr());
                if self.water_today() > 0 {
                    AppendMenuW(m, MF_STRING, Self::ID_UNDO as usize, w("Undo last glass").as_ptr());
                }
            }
            let dnd = CreatePopupMenu();
            let active = self.state.dnd_active(now_unix());
            AppendMenuW(dnd, MF_STRING, Self::ID_DND_1H as usize, w("For 1 hour").as_ptr());
            AppendMenuW(dnd, MF_STRING, Self::ID_DND_MORNING as usize, w("Until tomorrow morning").as_ptr());
            if active {
                AppendMenuW(dnd, MF_SEPARATOR, 0, std::ptr::null());
                AppendMenuW(dnd, MF_STRING, Self::ID_DND_OFF as usize, w("Turn off").as_ptr());
            }
            let dnd_label = match self.state.dnd_until.filter(|&t| t > now_unix()) {
                Some(t) => format!("Do not disturb (until {})", hm(t)),
                None => "Do not disturb".into(),
            };
            AppendMenuW(m, MF_POPUP | if active { MF_CHECKED } else { 0 }, dnd as usize, w(&dnd_label).as_ptr());
            AppendMenuW(
                m,
                MF_STRING | if self.settings.pet.always_on_top { MF_CHECKED } else { 0 },
                Self::ID_TOP as usize,
                w("Always on top").as_ptr(),
            );
            AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
            AppendMenuW(m, MF_STRING, Self::ID_SETTINGS as usize, w("Settings…").as_ptr());
            AppendMenuW(m, MF_STRING, Self::ID_QUIT as usize, w("Quit").as_ptr());
            m
        }
    }

    pub fn menu_command(&mut self, id: u32) {
        let now = now_unix();
        match id {
            Self::ID_TOGGLE => self.set_visible(!self.pet.visible),
            Self::ID_DRINK => {
                if !self.pet.visible {
                    self.set_visible(true);
                }
                self.drink();
            }
            Self::ID_UNDO => self.undo_drink(),
            Self::ID_DND_1H | Self::ID_DND_MORNING => {
                let until = if id == Self::ID_DND_1H {
                    now + 3600
                } else {
                    let morning = if self.settings.quiet.enabled { self.settings.quiet.end } else { TimeOfDay::hm(7, 0) };
                    schedule::next_time_of_day(morning, now + 3600, &Local)
                };
                self.state.dnd_until = Some(until);
                self.state_dirty = true;
                self.schedule_save();
                self.message(format!("Shh. I'll stay quiet until {}.", hm(until)), 3);
                self.anim(AnimKind::Nod);
                self.reschedule();
            }
            Self::ID_DND_OFF => {
                self.state.dnd_until = None;
                self.state_dirty = true;
                self.schedule_save();
                self.reschedule();
            }
            Self::ID_TOP => {
                self.settings.pet.always_on_top = !self.settings.pet.always_on_top;
                self.settings_dirty = true;
                self.schedule_save();
                self.apply_topmost();
                self.ui_invalidate();
            }
            Self::ID_SETTINGS => self.effects.push(Effect::OpenSettings),
            Self::ID_QUIT => self.effects.push(Effect::Quit),
            _ => {}
        }
    }

    // -----------------------------------------------------------------------------------------
    // Window / system state
    // -----------------------------------------------------------------------------------------

    pub fn set_visible(&mut self, visible: bool) {
        self.pet.visible = visible;
        self.state.pet_hidden = !visible;
        self.state_dirty = true;
        self.schedule_save();
        unsafe { ShowWindow(self.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
        if visible {
            self.pet.start_anim(AnimKind::PopIn);
            self.render();
            if !self.pending.is_empty() {
                self.set_timer(T_NEXT, 700);
            }
        } else {
            self.pet.bubble = None;
            self.pet.anim = None;
            self.pet.release_surface();
            self.kill_timer(T_ANIM);
            self.anim_timer = false;
        }
        self.arm_idle();
        self.arm_clock();
    }

    pub fn apply_topmost(&self) {
        let after = if self.settings.pet.always_on_top { HWND_TOPMOST } else { HWND_NOTOPMOST };
        unsafe { SetWindowPos(self.hwnd, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
    }

    pub fn on_session(&mut self, locked: bool) {
        if self.locked == locked {
            return;
        }
        self.locked = locked;
        if locked {
            self.kill_timer(T_IDLE);
            self.render();
        } else {
            self.away = false;
            self.kill_timer(T_AWAY);
            self.anim(AnimKind::WakeUp);
            self.reschedule();
            self.arm_idle();
        }
    }

    /// ABN_FULLSCREENAPP: a fullscreen app opened/closed. Verified against the shell's own state.
    pub fn on_fullscreen(&mut self, opened: bool) {
        let busy = opened && platform::user_busy_fullscreen();
        if busy == self.fullscreen {
            return;
        }
        self.fullscreen = busy;
        let hide = busy && self.settings.notify.pause_in_fullscreen && self.settings.pet.always_on_top && self.fullscreen_on_pet_monitor();
        if self.pet.visible {
            unsafe { ShowWindow(self.hwnd, if hide { SW_HIDE } else { SW_SHOWNOACTIVATE }) };
        }
        if !busy {
            self.render();
            self.reschedule();
        }
        self.arm_idle();
    }

    fn fullscreen_on_pet_monitor(&self) -> bool {
        use windows_sys::Win32::Graphics::Gdi::*;
        unsafe {
            let fg = GetForegroundWindow();
            if fg.is_null() {
                return true;
            }
            MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) == MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST)
        }
    }

    pub fn on_resume(&mut self) {
        self.pet.clamp_anchor(self.size());
        self.reschedule();
    }

    pub fn on_display_change(&mut self) {
        let pos = self.pet.anchor;
        if platform::monitor_at(pos.0 + 20, pos.1 + 20, true).is_none() {
            self.pet.anchor = Pet::default_anchor(self.size());
        }
        self.pet.clamp_anchor(self.size());
        self.render();
    }

    pub fn on_theme_change(&mut self) {
        platform::allow_dark_menus(self.palette().dark);
        self.render();
        self.ui_invalidate();
    }

    pub fn on_taskbar_created(&mut self) {
        let tip = self.tray_tip();
        self.tray.add(self.settings.pet.color.rgb(), !self.pending.is_empty(), &tip);
    }

    // -----------------------------------------------------------------------------------------
    // Settings changes & persistence
    // -----------------------------------------------------------------------------------------

    /// Applies a settings change immediately. `prev` is the settings before the change.
    pub fn settings_changed(&mut self, prev: &Settings) {
        let now = now_unix();
        // Newly created/edited reminders don't fire for times that already passed today.
        for r in &self.settings.reminders {
            let old = prev.reminders.iter().find(|o| o.id == r.id);
            let timing_changed = old.map_or(true, |o| o.time != r.time || o.days != r.days || o.date != r.date || (!o.enabled && r.enabled));
            if timing_changed {
                schedule::reminder_edited(&mut self.state, &r.id, now);
            }
        }
        self.state.prune(&self.settings);
        if prev.pet.always_on_top != self.settings.pet.always_on_top {
            self.apply_topmost();
        }
        if prev.pet.start_with_system != self.settings.pet.start_with_system && !self.dev_mode {
            platform::set_autostart(self.settings.pet.start_with_system);
        }
        if prev.pet.theme != self.settings.pet.theme {
            platform::allow_dark_menus(self.palette().dark);
        }
        if prev.pet.size != self.settings.pet.size {
            self.pet.clamp_anchor(self.size());
            self.state.desktop_pos = Some([self.pet.anchor.0, self.pet.anchor.1]);
        }
        if prev.water != self.settings.water {
            // Changing the schedule shouldn't instantly nag about the old one.
            if !self.state.water.anchor.is_some_and(|a| a > now - 60) && prev.water.interval_min != self.settings.water.interval_min {
                self.state.water.anchor = self.state.water.anchor.max(Some(now - 60));
            }
        }
        self.settings_dirty = true;
        self.state_dirty = true;
        self.schedule_save();
        self.reschedule();
        self.arm_idle();
        self.arm_clock();
    }

    pub fn schedule_save(&self) {
        self.set_timer(T_SAVE, 1_500);
    }

    pub fn flush_save(&mut self) {
        if self.settings_dirty {
            self.store.save_settings(&self.settings);
            self.settings_dirty = false;
        }
        if self.state_dirty {
            self.store.save_state(&self.state);
            self.state_dirty = false;
        }
    }

    pub fn shutdown(&mut self) {
        self.flush_save();
        self.tray.remove();
        platform::unregister_appbar(self.hwnd);
        unsafe { windows_sys::Win32::System::RemoteDesktop::WTSUnRegisterSessionNotification(self.hwnd) };
    }

    // -----------------------------------------------------------------------------------------
    // Developer / automation commands (WM_COPYDATA)
    // -----------------------------------------------------------------------------------------

    pub fn command(&mut self, cmd: &str) -> String {
        let mut parts = cmd.splitn(2, ' ');
        let verb = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();
        match verb {
            "show" => {
                self.set_visible(true);
                self.anim(AnimKind::Wave);
                "ok".into()
            }
            "quit" => {
                self.effects.push(Effect::Quit);
                "ok".into()
            }
            _ if !cfg!(feature = "devtools") => "unknown command".into(),
            "water" => {
                let d = Due { key: ItemKey::Water, occurrence: now_unix(), priority: Priority::Normal, late: false };
                self.pending.retain(|p| p.key != ItemKey::Water);
                self.pet.bubble = None;
                self.present(d, true);
                "ok".into()
            }
            "remind" => {
                let id = "dev-reminder".to_string();
                if !self.settings.reminders.iter().any(|r| r.id == id) {
                    self.settings.reminders.push(pip_core::settings::CustomReminder {
                        id: id.clone(),
                        title: if arg.is_empty() { "Stand-up meeting".into() } else { arg.into() },
                        important: true,
                        enabled: false,
                        ..Default::default()
                    });
                }
                self.pending.retain(|p| p.key != ItemKey::Custom(id.clone()));
                self.pet.bubble = None;
                let d = Due { key: ItemKey::Custom(id), occurrence: now_unix(), priority: Priority::High, late: false };
                self.present(d, false);
                "ok".into()
            }
            "click" => {
                self.click_creature();
                "ok".into()
            }
            "hover" => {
                let c = self.tooltip_card();
                self.pet.show_bubble(c);
                self.render();
                "ok".into()
            }
            "act" => {
                let a = match arg {
                    "done" => Action::Done,
                    "snooze" => Action::Snooze,
                    "dismiss" => Action::Dismiss,
                    "drink" => Action::Drink,
                    _ => Action::Close,
                };
                self.act(a);
                "ok".into()
            }
            "anim" => {
                let k = match arg {
                    "hop" => AnimKind::Hop,
                    "wave" => AnimKind::Wave,
                    "celebrate" => AnimKind::Celebrate,
                    "drink" => AnimKind::Drink,
                    "look" => AnimKind::LookAround,
                    "stretch" => AnimKind::Stretch,
                    "wiggle" => AnimKind::Wiggle,
                    _ => AnimKind::Blink,
                };
                self.anim(k);
                "ok".into()
            }
            "freeze" => {
                // Render one animation at a fixed progress (for visual snapshots).
                let mut it = arg.split_whitespace();
                let k = it.next().unwrap_or("hop");
                let t: f32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.5);
                let kind = match k {
                    "celebrate" => AnimKind::Celebrate,
                    "drink" => AnimKind::Drink,
                    "wave" => AnimKind::Wave,
                    "stretch" => AnimKind::Stretch,
                    "blink" => AnimKind::Blink,
                    _ => AnimKind::Hop,
                };
                let offset = (kind.duration_ms() as f32 * t) as u64;
                self.pet.anim = Some((kind, tick_ms().saturating_sub(offset) - 0));
                self.render();
                self.pet.anim = None;
                self.kill_timer(T_ANIM);
                self.anim_timer = false;
                "ok".into()
            }
            "sleep" => {
                self.away = !self.away;
                self.render();
                format!("away={}", self.away)
            }
            "settings" => {
                self.effects.push(Effect::OpenSettings);
                if let Ok(p) = arg.parse::<usize>() {
                    self.ui_page(p);
                }
                "ok".into()
            }
            "page" => {
                self.ui_page(arg.parse().unwrap_or(0));
                "ok".into()
            }
            "close-settings" => {
                self.effects.push(Effect::CloseSettings);
                "ok".into()
            }
            "hide" => {
                self.set_visible(false);
                "ok".into()
            }
            "ui-click" => {
                let v: Vec<f32> = arg.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                if v.len() == 2 {
                    self.ui_click(v[0], v[1]);
                }
                "ok".into()
            }
            "ui-type" => {
                for ch in arg.chars() {
                    self.ui_char(ch as u32);
                }
                if arg.is_empty() {
                    self.ui_char(0x0D);
                }
                "ok".into()
            }
            "snap" => self.dev_snapshot(arg),
            "stats" => {
                let u = platform::process_usage();
                format!(
                    "{{\"private\":{},\"ws\":{},\"cpu_ms\":{},\"handles\":{},\"render_us\":{},\"frames\":{},\"startup_ms\":{},\"pending\":{},\"bubble\":\"{:?}\",\"expr\":\"{:?}\",\"water\":{},\"x\":{},\"y\":{},\"w\":{},\"h\":{}}}",
                    u.private_bytes, u.working_set, u.cpu_ms, u.handles, self.pet.render_us, self.pet.frames, self.startup_ms,
                    self.pending.len(), self.pet.bubble_kind(), self.expr(), self.water_today(),
                    self.pet.win.l, self.pet.win.t, self.pet.win.w(), self.pet.win.h()
                )
            }
            _ => "unknown command".into(),
        }
    }

    #[cfg(feature = "devtools")]
    fn dev_snapshot(&mut self, dir: &str) -> String {
        let save = |data: Vec<u8>, w: u32, h: u32, path: std::path::PathBuf| -> bool {
            tiny_skia::IntSize::from_wh(w, h)
                .and_then(|size| tiny_skia::Pixmap::from_vec(data, size))
                .map(|p| p.save_png(&path).is_ok())
                .unwrap_or(false)
        };
        let dir = std::path::PathBuf::from(dir);
        let (d, w, h) = self.pet.snapshot_rgba();
        let mut out = format!("pet={}", save(d, w, h, dir.join("pet.png")));
        if let Some(ui) = &mut self.ui {
            let (d, w, h) = ui.snapshot_rgba();
            out += &format!(" settings={}", save(d, w, h, dir.join("settings.png")));
        }
        out
    }

    #[cfg(not(feature = "devtools"))]
    fn dev_snapshot(&mut self, _dir: &str) -> String {
        "devtools disabled".into()
    }
}

impl App {
    pub fn mark_settings_dirty(&mut self) {
        self.settings_dirty = true;
    }
}
