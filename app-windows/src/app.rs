//! The application controller: owns settings/state, turns core decisions into what the creature
//! does, and keeps exactly the timers that are needed — nothing ticks unless something is due.

use crate::pet::{Action, BubbleKind, Card, Pet, View};
use crate::platform::{self, now_unix, tick_ms};
use crate::render::creature::{Acc, AnimKind, Expr};
use crate::render::ui::BtnStyle;
use crate::render::Palette;
use crate::settings_ui::SettingsUi;
use crate::storage::Storage;
use crate::calsync::{self, FeedJob, FeedMeta};
use crate::tray::Tray;
use pip_core::agenda;
use pip_core::calendar::Event;
use pip_core::links::{self, LinkCheck, Provider};
use std::collections::HashMap;
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
pub const T_CALSYNC: usize = 11;
pub const T_CLIPWAIT: usize = 12;
pub const T_CLIPREAD: usize = 13;
pub const T_MAIL: usize = 14;

/// Clipboard watching lasts at most this long after starting to add a calendar.
pub const CLIPWAIT_MS: u32 = 10 * 60_000;

/// "Add a calendar" in progress: Pip waits for the link to be copied in the browser.
pub struct CalWizard {
    pub provider: Provider,
    /// Last problem noticed with a copied link, shown in Settings.
    pub hint: Option<String>,
}

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
    /// Calendar occurrences for the current two-week window (all feeds).
    pub events: Vec<Event>,
    pub feed_meta: HashMap<String, FeedMeta>,
    sync_running: bool,
    sync_failures: u32,
    secrets_dirty: bool,
    pub cal_wizard: Option<CalWizard>,
    /// The provider picker is showing in Settings › Calendars.
    pub cal_picking: bool,
    /// Page to show when Settings opens next.
    pub open_page: Option<usize>,
    /// Encrypted-at-rest secrets, decrypted in memory (calendar links, Google keys, sign-ins).
    pub secrets: std::collections::BTreeMap<String, String>,
    clip_listening: bool,
    /// Gmail setup is waiting for the Client ID / secret to be copied.
    pub gmail_setup: bool,
    pub mail_access: HashMap<String, (String, i64)>,
    pub mail_status: HashMap<String, MailStatus>,
    mail_running: bool,
    pub auth_running: bool,
    pub auth_error: Option<String>,
    auth_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Last consent URL (lets automated tests play the browser).
    pub last_auth_url: String,
}

#[derive(Clone, Debug, Default)]
pub struct MailStatus {
    pub checked_at: i64,
    pub error: Option<String>,
    pub signed_out: bool,
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
            events: Vec::new(),
            feed_meta: HashMap::new(),
            sync_running: false,
            sync_failures: 0,
            secrets_dirty: false,
            cal_wizard: None,
            cal_picking: false,
            open_page: None,
            secrets: Default::default(),
            clip_listening: false,
            gmail_setup: false,
            mail_access: HashMap::new(),
            mail_status: HashMap::new(),
            mail_running: false,
            auth_running: false,
            auth_error: None,
            auth_cancel: None,
            last_auth_url: String::new(),
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
        // Calendars: secret links from the encrypted store, cached events right away (offline),
        // then a network refresh once the desktop has settled.
        self.secrets = crate::secrets::load(self.store.dir());
        let secrets = self.secrets.clone();
        for c in &mut self.settings.dates.calendars {
            c.url = secrets.get(&format!("cal:{}", c.id)).cloned().unwrap_or_default();
            self.feed_meta.insert(c.id.clone(), calsync::load_meta(self.store.dir(), &c.id));
        }
        self.start_sync(false);
        self.set_timer(T_CALSYNC, 20_000);
        if !self.settings.email.accounts.is_empty() {
            self.set_timer(T_MAIL, 25_000); // first email check once the desktop has settled
        }
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
                ItemKey::Date(_) => Acc::Heart,
                _ => Acc::Alert,
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
            self.set_timer(T_MAIL, 3_000); // catch up on email now that they're back
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
            T_CALSYNC => {
                self.kill_timer(T_CALSYNC);
                self.start_sync(true);
            }
            T_CLIPREAD => {
                self.kill_timer(T_CLIPREAD);
                self.read_clipboard_for_wizard();
            }
            T_CLIPWAIT => {
                self.kill_timer(T_CLIPWAIT);
                self.end_cal_wizard();
                self.gmail_setup = false;
                self.update_clip_listener();
            }
            T_MAIL => {
                self.kill_timer(T_MAIL);
                self.check_mail(false);
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
            self.state.prune_events(now);
            self.state_dirty = true;
            self.schedule_save();
            self.start_sync(false); // slide the two-week window
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

        let due = schedule::due_items_in(&self.settings, &self.state, &self.events, now, &tz);
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

        let mut next = schedule::next_event_in(&self.settings, &self.state, &self.events, now, &tz);
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
            ItemKey::Date(id) => {
                let Some(item) = self.settings.dates.items.iter().find(|x| &x.id == id) else { return };
                let today = local(&Local, now_unix()).date();
                let Some(on) = item.next_on_or_after(today) else { return };
                let wd = pip_core::time::weekday_name(chrono::Datelike::weekday(&on));
                messages::date_reminder(item.kind, &item.describe(on), (on - today).num_days(), wd, &name)
            }
            ItemKey::Event(key) => {
                let Some(e) = self.events.iter().find(|e| &e.key() == key) else { return };
                let mins = (e.start - now_unix()) / 60;
                (e.title.clone(), messages::event_body(e.all_day, &hm(e.start), mins))
            }
            ItemKey::Mail => {
                let (title, lines) = messages::mail_card(&self.state.mail_pending);
                if lines.is_empty() {
                    return;
                }
                (title, lines.join("
"))
            }
            ItemKey::Brief => {
                let now = now_unix();
                let l = local(&Local, now);
                let lines = agenda::brief_lines(&self.settings, &self.events, l.date(), &Local, hm);
                if lines.is_empty() {
                    return;
                }
                (messages::brief_title(&name, l.hour()), lines.join("\n"))
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
        let mut card = Card::new(BubbleKind::Notify);
        for line in p.body.split('\n') {
            card = card.line(line, matches!(p.key, ItemKey::Custom(_)));
        }
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
            ItemKey::Custom(_) | ItemKey::Date(_) => {
                card.buttons.push(("Done".into(), Action::Done, BtnStyle::Primary));
                card.buttons.push((snooze, Action::Snooze, BtnStyle::Secondary));
                card.buttons.push(("Dismiss".into(), Action::Dismiss, BtnStyle::Ghost));
            }
            ItemKey::Event(_) => {
                card.buttons.push(("Got it".into(), Action::Done, BtnStyle::Primary));
                card.buttons.push((snooze, Action::Snooze, BtnStyle::Secondary));
            }
            ItemKey::Brief => card.buttons.push(("Thanks!".into(), Action::Done, BtnStyle::Primary)),
            ItemKey::Mail => {
                card.buttons.push(("Open Gmail".into(), Action::Open, BtnStyle::Primary));
                card.buttons.push(("Got it".into(), Action::Done, BtnStyle::Secondary));
                card.buttons.push((snooze, Action::Snooze, BtnStyle::Ghost));
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
            Action::Done | Action::Snooze | Action::Dismiss | Action::Open => {
                if action == Action::Open {
                    self.open_mail();
                }
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
                    (ItemKey::Date(id), Action::Snooze) => {
                        schedule::snooze_key(&mut self.state, schedule::date_state_key(id), now + self.settings.notify.snooze_min as i64 * 60);
                    }
                    (ItemKey::Date(id), _) => schedule::ack_date(&mut self.state, id, p.occurrence),
                    (ItemKey::Event(key), Action::Snooze) => {
                        schedule::snooze_key(&mut self.state, schedule::event_state_key(key), now + self.settings.notify.snooze_min as i64 * 60);
                    }
                    (ItemKey::Event(key), _) => schedule::ack_event(&mut self.state, key, now),
                    (ItemKey::Brief, _) => schedule::ack_brief(&mut self.state, local(&Local, now).date()),
                    (ItemKey::Mail, Action::Snooze) => {
                        schedule::snooze_key(&mut self.state, schedule::MAIL_STATE_KEY.to_string(), now + self.settings.notify.snooze_min as i64 * 60);
                    }
                    (ItemKey::Mail, _) => schedule::ack_mail(&mut self.state),
                }
                if matches!(p.key, ItemKey::Brief | ItemKey::Event(_) | ItemKey::Mail) && matches!(action, Action::Done | Action::Open) {
                    // Acknowledging information isn't an accomplishment — no confetti.
                    self.pet.bubble = None;
                    self.kill_timer(T_BUBBLE);
                    self.state_dirty = true;
                    self.schedule_save();
                    self.anim(AnimKind::Nod);
                    if !self.pending.is_empty() {
                        self.set_timer(T_NEXT, 500);
                    }
                    self.reschedule();
                    return;
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
        // Today's agenda: holidays, dates, then events still ahead (at most four lines).
        let agenda: Vec<String> = agenda::agenda_for_day(s, &self.events, l.date(), &Local)
            .into_iter()
            .filter(|a| a.time.is_none_or(|t| t + 900 > now))
            .take(4)
            .map(|a| match a.time {
                Some(t) => format!("{}  {}", hm(t), a.text),
                None => a.text,
            })
            .collect();
        for line in agenda {
            card = card.line(line, false);
        }
        match self.state.mail_pending.len() {
            0 => {}
            1 => card = card.line("1 important email waiting", false),
            n => card = card.line(format!("{n} important emails waiting"), false),
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
    pub const ID_ADD_CAL: u32 = 131;
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
            if self.settings.dates.calendars.is_empty() {
                AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
                AppendMenuW(m, MF_STRING, Self::ID_ADD_CAL as usize, w("Add a calendar…").as_ptr());
            }
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
            Self::ID_ADD_CAL => {
                self.open_page = Some(crate::settings_ui::CALENDARS_PAGE);
                self.cal_picking = true;
                self.effects.push(Effect::OpenSettings);
                if let Some(ui) = &mut self.ui {
                    ui.page = crate::settings_ui::page_at(crate::settings_ui::CALENDARS_PAGE);
                }
                self.ui_invalidate();
            }
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
            self.set_timer(T_MAIL, 5_000);
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
        // Calendar subscriptions: re-encrypt links, drop caches of removed feeds, refresh soon.
        let (old_c, new_c) = (&prev.dates.calendars, &self.settings.dates.calendars);
        let fetch_changed = old_c.len() != new_c.len()
            || old_c.iter().zip(new_c).any(|(a, b)| a.id != b.id || a.url != b.url || a.enabled != b.enabled);
        if old_c.iter().map(|c| (&c.id, &c.url)).ne(new_c.iter().map(|c| (&c.id, &c.url))) {
            self.secrets_dirty = true;
        }
        for gone in old_c.iter().filter(|o| !new_c.iter().any(|n| n.id == o.id)) {
            calsync::forget(self.store.dir(), &gone.id);
            self.feed_meta.remove(&gone.id);
            self.events.retain(|e| e.feed_id != gone.id);
        }
        if fetch_changed {
            self.set_timer(T_CALSYNC, 1_500);
        }
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
        if self.secrets_dirty {
            let map = self.secrets_map();
            if !crate::secrets::save(self.store.dir(), &map) {
                self.store.log("could not save encrypted calendar links");
            }
            self.secrets_dirty = false;
        }
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
                if arg == "^V" {
                    self.ui_char(0x16);
                    return "ok".into();
                }
                for ch in arg.chars() {
                    self.ui_char(ch as u32);
                }
                if arg.is_empty() {
                    self.ui_char(0x0D);
                }
                "ok".into()
            }
            "add-cal" => {
                let prev = self.settings.clone();
                let id = format!("c{:x}{:x}", now_unix(), tick_ms() & 0xffff);
                self.settings.dates.calendars.push(pip_core::calendar::CalendarFeed { id: id.clone(), url: arg.to_string(), ..Default::default() });
                self.settings_changed(&prev);
                id
            }
            "add-date" => {
                // add-date <month> <day> <year|-> <lead-days> <title...>
                let p: Vec<&str> = arg.splitn(5, ' ').collect();
                if p.len() < 5 {
                    return "usage: add-date <m> <d> <year|-> <lead> <title>".into();
                }
                let prev = self.settings.clone();
                self.settings.dates.items.push(pip_core::dates::ImportantDate {
                    id: format!("d{:x}{}", now_unix(), self.settings.dates.items.len()),
                    title: p[4].to_string(),
                    month: p[0].parse().unwrap_or(1),
                    day: p[1].parse().unwrap_or(1),
                    year: p[2].parse().ok(),
                    lead_days: p[3].parse().unwrap_or(0),
                    ..Default::default()
                });
                self.settings_changed(&prev);
                "ok".into()
            }
            "wizard" => {
                self.effects.push(Effect::OpenSettings);
                let p = match arg { "google" => Provider::Google, "outlook" => Provider::Outlook, "icloud" => Provider::ICloud, _ => Provider::Other };
                self.begin_cal_wizard(p);
                "ok".into()
            }
            "menu" => {
                self.menu_command(arg.parse().unwrap_or(0));
                "ok".into()
            }
            "gmail-setup" => {
                self.begin_gmail_setup();
                format!("keys={}", self.google_keys().is_some())
            }
            "gmail-signin" => {
                self.start_gmail_signin();
                self.last_auth_url.clone()
            }
            "mail-check" => {
                self.check_mail(true);
                "started".into()
            }
            "mail" => {
                let st: Vec<String> = self.mail_status.iter().map(|(k, v)| format!("{k}: checked={} err={:?} signed_out={}", v.checked_at, v.error, v.signed_out)).collect();
                format!(
                    "accounts={:?} running={} auth_running={} auth_error={:?} pending={:?} history={:?}
{}",
                    self.settings.email.accounts.iter().map(|a| a.address.clone()).collect::<Vec<_>>(),
                    self.mail_running, self.auth_running, self.auth_error,
                    self.state.mail_pending.iter().map(|m| format!("{}: {}", m.sender(), m.subject)).collect::<Vec<_>>(),
                    self.state.mail.values().map(|m| m.history_id.clone()).collect::<Vec<_>>(),
                    st.join("
")
                )
            }
            "sync" => {
                self.start_sync(true);
                "started".into()
            }
            "events" => {
                let feeds: Vec<String> = self
                    .feed_meta
                    .iter()
                    .map(|(id, m)| format!("{id}: synced_at={} events={} error={:?} name={:?} etag={:?}", m.synced_at, m.events, m.error, m.name, m.etag))
                    .collect();
                let evs: Vec<String> = self.events.iter().take(8).map(|e| format!("{} {} {}", hm(e.start), if e.all_day { "(all day)" } else { "" }, e.title)).collect();
                format!("running={} total={}\n{}\n{}", self.sync_running, self.events.len(), feeds.join("\n"), evs.join("\n"))
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

// ---------------------------------------------------------------------------------------------
// Calendar subscriptions
// ---------------------------------------------------------------------------------------------

impl App {
    /// Starts a sync job (`online = false` only re-reads the on-disk caches, e.g. at midnight).
    pub fn start_sync(&mut self, online: bool) {
        if self.sync_running {
            return;
        }
        let jobs: Vec<FeedJob> = self
            .settings
            .dates
            .calendars
            .iter()
            .filter(|c| c.enabled)
            .map(|c| FeedJob { id: c.id.clone(), url: c.url.clone() })
            .collect();
        if jobs.is_empty() {
            if !self.events.is_empty() {
                self.events.clear();
                self.reschedule();
            }
            return;
        }
        self.sync_running = true;
        let today = local(&Local, now_unix()).date();
        calsync::start(self.hwnd, self.store.dir().to_path_buf(), jobs, online, today);
    }

    pub fn on_calsync(&mut self, result: Box<calsync::SyncResult>) {
        self.sync_running = false;
        let mut failures = 0;
        for (id, meta) in result.feeds {
            if meta.error.is_some() {
                failures += 1;
            }
            // Adopt the calendar's own name for feeds still called the default.
            if let (Some(name), Some(c)) = (&meta.name, self.settings.dates.calendars.iter_mut().find(|c| c.id == id)) {
                if c.name == "Calendar" || c.name.trim().is_empty() {
                    c.name = name.chars().take(40).collect();
                    self.settings_dirty = true;
                    self.schedule_save();
                }
            }
            self.feed_meta.insert(id, meta);
        }
        self.events = result.events;
        if result.online {
            // Hourly when healthy; back off to 15, 30, 60, 120 minutes while failing.
            self.sync_failures = if failures > 0 { self.sync_failures + 1 } else { 0 };
            let minutes = if self.sync_failures == 0 { 60 } else { (15u32 << (self.sync_failures - 1).min(3)).min(120) };
            self.set_timer(T_CALSYNC, minutes * 60_000);
        }
        self.reschedule();
        self.ui_invalidate();
    }

    pub fn secrets_map(&self) -> std::collections::BTreeMap<String, String> {
        // Everything that isn't a calendar link (Google keys, sign-ins) plus the current links.
        let mut m: std::collections::BTreeMap<String, String> = self.secrets.iter().filter(|(k, _)| !k.starts_with("cal:")).map(|(k, v)| (k.clone(), v.clone())).collect();
        m.extend(self.settings
            .dates
            .calendars
            .iter()
            .filter(|c| !c.url.trim().is_empty())
            .map(|c| (format!("cal:{}", c.id), c.url.trim().to_string())));
        m
    }
}

// ---------------------------------------------------------------------------------------------
// Guided "add a calendar": open the right page, then pick the link up from the clipboard.
// The clipboard is only watched during this flow (max 10 minutes), via Windows' change
// notification — no polling, and anything that isn't a calendar link is ignored and forgotten.
// ---------------------------------------------------------------------------------------------

impl App {
    pub fn provider_page(provider: Provider) -> Option<&'static str> {
        match provider {
            Provider::Google => Some("https://calendar.google.com/calendar/r/settings"),
            Provider::Outlook => Some("https://outlook.live.com/mail/0/options/calendar/SharedCalendars"),
            Provider::ICloud => Some("https://www.icloud.com/calendar/"),
            Provider::Other => None,
        }
    }

    pub fn begin_cal_wizard(&mut self, provider: Provider) {
        self.cal_picking = false;
        self.cal_wizard = Some(CalWizard { provider, hint: None });
        self.update_clip_listener();
        self.set_timer(T_CLIPWAIT, CLIPWAIT_MS);
        if let Some(url) = App::provider_page(provider) {
            platform::open_url(url);
        }
        // Maybe it's already on the clipboard — but only take a link from the chosen provider, so an
        // old unrelated link lying around isn't added by surprise.
        let text = platform::clipboard_text().filter(|t| match links::check(t, false) {
            LinkCheck::Calendar { provider: p, .. } => p == provider,
            _ => false,
        });
        self.try_calendar_text(text, provider == Provider::Other, true);
        self.ui_invalidate();
    }

    pub fn end_cal_wizard(&mut self) {
        self.cal_wizard = None;
        self.update_clip_listener();
        self.cal_picking = false;
        self.ui_invalidate();
    }

    /// WM_CLIPBOARDUPDATE while adding a calendar. Reading is deferred: the app that just copied
    /// may open the clipboard again right away (.NET, some browsers), and grabbing it in that
    /// instant would make *their* copy fail. Bursts of updates coalesce into one read.
    pub fn on_clipboard(&mut self) {
        if self.cal_wizard.is_some() || self.gmail_setup {
            self.set_timer(T_CLIPREAD, 200);
        }
    }

    fn read_clipboard_for_wizard(&mut self) {
        if self.gmail_setup {
            self.capture_google_keys();
        }
        let Some(w) = &self.cal_wizard else { return };
        let lenient = w.provider == Provider::Other;
        let text = platform::clipboard_text();
        self.try_calendar_text(text, lenient, false);
    }

    /// The "Paste link" button: take whatever is on the clipboard now.
    pub fn paste_calendar_now(&mut self) {
        let text = platform::clipboard_text();
        if !self.try_calendar_text(text, true, false) {
            if let Some(w) = &mut self.cal_wizard {
                w.hint.get_or_insert_with(|| "Copy the calendar link first, then press Paste link.".into());
            }
            self.ui_invalidate();
        }
    }

    /// Adds the calendar if `text` holds a usable link. Returns true when one was added.
    fn try_calendar_text(&mut self, text: Option<String>, lenient: bool, quiet: bool) -> bool {
        let Some(text) = text else { return false };
        match links::check(&text, lenient) {
            LinkCheck::Calendar { url, provider } => {
                if self.settings.dates.calendars.iter().any(|c| c.url.trim() == url) {
                    if let Some(w) = &mut self.cal_wizard {
                        w.hint = Some("That calendar is already added.".into());
                    }
                    self.ui_invalidate();
                    return false;
                }
                let prev = self.settings.clone();
                let id = format!("c{:x}{:04x}", now_unix(), tick_ms() & 0xffff);
                self.settings.dates.calendars.push(pip_core::calendar::CalendarFeed {
                    id,
                    url,
                    updated_at: now_unix(),
                    ..Default::default()
                });
                self.settings_changed(&prev); // encrypts the link and syncs in a moment
                self.end_cal_wizard();
                let what = provider.your_calendar();
                if !self.pet.visible {
                    self.set_visible(true);
                }
                self.message(format!("Got it! I'll keep an eye on {what}. Important events will get a gentle nudge."), 6);
                self.anim(AnimKind::Celebrate);
                true
            }
            LinkCheck::Mistake(m) => {
                if let Some(w) = &mut self.cal_wizard {
                    w.hint = Some(m.clone());
                }
                if !quiet {
                    self.message(m, 8);
                    self.anim(AnimKind::Wiggle);
                }
                self.ui_invalidate();
                false
            }
            LinkCheck::NotALink => false,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Important email (Gmail, metadata-only)
// ---------------------------------------------------------------------------------------------

pub const GMAIL_ID_KEY: &str = "gmail:client_id";
pub const GMAIL_SECRET_KEY: &str = "gmail:client_secret";

fn refresh_key(account_id: &str) -> String {
    format!("gmail:{account_id}:refresh")
}

impl App {
    /// One clipboard listener shared by the calendar and Gmail setup flows, on only while needed.
    pub fn update_clip_listener(&mut self) {
        let want = self.cal_wizard.is_some() || self.gmail_setup;
        if want == self.clip_listening {
            if want {
                self.set_timer(T_CLIPWAIT, CLIPWAIT_MS);
            }
            return;
        }
        unsafe {
            if want {
                windows_sys::Win32::System::DataExchange::AddClipboardFormatListener(self.hwnd);
            } else {
                windows_sys::Win32::System::DataExchange::RemoveClipboardFormatListener(self.hwnd);
            }
        }
        self.clip_listening = want;
        if want {
            self.set_timer(T_CLIPWAIT, CLIPWAIT_MS);
        } else {
            self.kill_timer(T_CLIPWAIT);
            self.kill_timer(T_CLIPREAD);
        }
    }

    pub fn google_keys(&self) -> Option<crate::oauth::ClientKeys> {
        Some(crate::oauth::ClientKeys { id: self.secrets.get(GMAIL_ID_KEY)?.clone(), secret: self.secrets.get(GMAIL_SECRET_KEY)?.clone() })
    }

    pub fn begin_gmail_setup(&mut self) {
        self.gmail_setup = true;
        self.auth_error = None;
        self.update_clip_listener();
        self.capture_google_keys(); // maybe already copied
        self.ui_invalidate();
    }

    pub fn end_gmail_setup(&mut self) {
        self.gmail_setup = false;
        self.update_clip_listener();
        self.ui_invalidate();
    }

    /// Takes a Client ID and/or secret from the clipboard (single values or the whole JSON).
    pub fn capture_google_keys(&mut self) {
        let Some(text) = platform::clipboard_text() else { return };
        let (id, secret) = pip_core::email::gmail::find_client_keys(&text);
        let mut changed = false;
        if let Some(id) = id {
            changed |= self.secrets.insert(GMAIL_ID_KEY.into(), id.clone()).as_deref() != Some(id.as_str());
        }
        if let Some(s) = secret {
            changed |= self.secrets.insert(GMAIL_SECRET_KEY.into(), s.clone()).as_deref() != Some(s.as_str());
        }
        if changed {
            self.secrets_dirty = true;
            self.schedule_save();
            if self.google_keys().is_some() {
                self.end_gmail_setup();
                self.message("Got both keys! Now click \u{201c}Sign in with Google\u{201d}.".into(), 6);
                self.anim(AnimKind::Hop);
            } else {
                self.message("Got it \u{2014} now copy the other one.".into(), 4);
                self.anim(AnimKind::Nod);
            }
        }
        self.ui_invalidate();
    }

    pub fn forget_google_keys(&mut self) {
        self.secrets.remove(GMAIL_ID_KEY);
        self.secrets.remove(GMAIL_SECRET_KEY);
        self.secrets_dirty = true;
        self.schedule_save();
        self.ui_invalidate();
    }

    pub fn start_gmail_signin(&mut self) {
        let Some(keys) = self.google_keys() else { return };
        if let Some(c) = self.auth_cancel.take() {
            c.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        match crate::oauth::prepare(&keys) {
            Ok((pending, url)) => {
                let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                self.auth_cancel = Some(cancel.clone());
                self.auth_running = true;
                self.auth_error = None;
                platform::open_url(&url);
                self.last_auth_url = url;
                crate::oauth::finish_in_background(self.hwnd, pending, cancel);
            }
            Err(e) => self.auth_error = Some(e),
        }
        self.ui_invalidate();
    }

    pub fn cancel_gmail_signin(&mut self) {
        if let Some(c) = self.auth_cancel.take() {
            c.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.auth_running = false;
        self.ui_invalidate();
    }

    pub fn on_mailauth(&mut self, result: Box<crate::oauth::AuthResult>) {
        use crate::oauth::AuthResult;
        self.auth_running = false;
        self.auth_cancel = None;
        match *result {
            AuthResult::Ok { refresh, access, expires_at, address, history_id } => {
                let prev = self.settings.clone();
                let id = match self.settings.email.accounts.iter().find(|a| a.address.eq_ignore_ascii_case(&address)) {
                    Some(a) => a.id.clone(),
                    None => {
                        let id = format!("g{:x}{:04x}", now_unix(), tick_ms() & 0xffff);
                        self.settings.email.accounts.push(pip_core::email::EmailAccount {
                            id: id.clone(),
                            address: address.clone(),
                            enabled: true,
                            updated_at: now_unix(),
                        });
                        id
                    }
                };
                self.secrets.insert(refresh_key(&id), refresh);
                self.secrets_dirty = true;
                // Only mail arriving from now on is considered: no flood of old messages.
                let ms = self.state.mail.entry(id.clone()).or_default();
                ms.history_id = Some(history_id);
                ms.last_check = now_unix();
                self.mail_access.insert(id.clone(), (access, expires_at));
                self.mail_status.insert(id, MailStatus { checked_at: now_unix(), ..Default::default() });
                self.state_dirty = true;
                self.settings_changed(&prev);
                if !self.pet.visible {
                    self.set_visible(true);
                }
                self.message(format!("Connected to {address}! I'll give you a gentle nudge when something important arrives."), 7);
                self.anim(AnimKind::Celebrate);
                self.schedule_mail();
            }
            AuthResult::Err(e) => {
                self.auth_error = Some(e.clone());
                self.message(e, 8);
            }
            AuthResult::Cancelled => {}
        }
        self.ui_invalidate();
    }

    pub fn disconnect_mail(&mut self, account_id: &str) {
        let prev = self.settings.clone();
        self.settings.email.accounts.retain(|a| a.id != account_id);
        if let Some(token) = self.secrets.remove(&refresh_key(account_id)) {
            crate::oauth::revoke_in_background(token);
        }
        self.secrets_dirty = true;
        self.state.mail.remove(account_id);
        self.state.mail_pending.retain(|m| m.account_id != account_id);
        self.mail_access.remove(account_id);
        self.mail_status.remove(account_id);
        self.state_dirty = true;
        self.settings_changed(&prev);
        self.ui_invalidate();
    }

    /// Next check: every few minutes while you're around; every 15 minutes during quiet hours.
    pub fn schedule_mail(&mut self) {
        if self.settings.email.accounts.iter().all(|a| !a.enabled) || self.google_keys().is_none() {
            self.kill_timer(T_MAIL);
            return;
        }
        let minutes = if self.night { 15 } else { self.settings.email.check_minutes.max(1) };
        self.set_timer(T_MAIL, minutes * 60_000);
    }

    /// Starts a check. `force` also checks while away/locked (e.g. "Check now").
    pub fn check_mail(&mut self, force: bool) {
        if self.mail_running {
            return;
        }
        if !force && (self.locked || self.away) {
            // Nobody to tell; unlock / coming back triggers a check.
            self.kill_timer(T_MAIL);
            return;
        }
        let Some(keys) = self.google_keys() else { return };
        let jobs: Vec<crate::mailsync::MailJob> = self
            .settings
            .email
            .accounts
            .iter()
            .filter(|a| a.enabled && !self.mail_status.get(&a.id).is_some_and(|s| s.signed_out))
            .filter_map(|a| {
                Some(crate::mailsync::MailJob {
                    account_id: a.id.clone(),
                    refresh: self.secrets.get(&refresh_key(&a.id))?.clone(),
                    access: self.mail_access.get(&a.id).cloned(),
                    history_id: self.state.mail.get(&a.id).and_then(|m| m.history_id.clone()),
                })
            })
            .collect();
        if jobs.is_empty() {
            return;
        }
        self.mail_running = true;
        crate::mailsync::start(self.hwnd, keys, self.settings.email.clone(), jobs);
    }

    pub fn on_mailsync(&mut self, results: Box<Vec<crate::mailsync::MailResult>>) {
        self.mail_running = false;
        let now = now_unix();
        let mut found = Vec::new();
        for r in *results {
            match &r.access {
                Some(a) => {
                    self.mail_access.insert(r.account_id.clone(), a.clone());
                }
                None => {
                    self.mail_access.remove(&r.account_id);
                }
            }
            let ms = self.state.mail.entry(r.account_id.clone()).or_default();
            if r.history_id.is_some() {
                ms.history_id = r.history_id.clone();
            }
            if r.error.is_none() {
                ms.last_check = now;
            }
            let status = self.mail_status.entry(r.account_id.clone()).or_default();
            status.error = r.error.clone();
            status.signed_out = r.signed_out;
            if r.error.is_none() {
                status.checked_at = now;
            }
            if r.signed_out {
                let address = self.settings.email.accounts.iter().find(|a| a.id == r.account_id).map(|a| a.address.clone()).unwrap_or_default();
                self.message(format!("Google signed me out of {address}. Open Settings \u{203a} Email to sign in again."), 8);
            }
            found.extend(r.important);
        }
        self.state_dirty = true;
        self.schedule_save();
        if self.state.add_mail(found) > 0 {
            // New arrivals refresh a bubble that's already showing.
            let showing_mail = self.pending.first().is_some_and(|p| p.key == ItemKey::Mail) && self.pet.bubble_kind() == Some(BubbleKind::Notify);
            self.pending.retain(|p| p.key != ItemKey::Mail);
            if showing_mail {
                self.pet.bubble = None;
            }
        }
        self.reschedule();
        self.schedule_mail();
        self.ui_invalidate();
    }

    /// "Open Gmail": the message itself if there's one, otherwise the inbox.
    pub fn open_mail(&mut self) {
        let newest = self.state.mail_pending.iter().max_by_key(|m| m.received).cloned();
        if let Some(m) = newest {
            let address = self.settings.email.accounts.iter().find(|a| a.id == m.account_id).map(|a| a.address.clone()).unwrap_or_default();
            let single = self.state.mail_pending.len() == 1;
            platform::open_url(&pip_core::email::gmail::web_link(&address, single.then_some(m.id.as_str())));
        }
    }
}
