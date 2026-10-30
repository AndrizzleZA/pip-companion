//! Settings window. Custom-drawn with the same renderer as the pet, so it matches the creature's
//! look and adds no UI framework. It exists only while open; closing frees all of it.

use crate::app::{hm, App, Effect};
use crate::platform::{self, tick_ms, Usage};
use crate::render::creature::{self, Expr, Pose};
use crate::render::text::{self, fonts};
use crate::render::ui::{self, BtnStyle, R};
use crate::render::{color, fill_rrect, stroke_rrect, Palette};
use crate::surface::Surface;
use pip_core::schedule;
use pip_core::dates::{DateKind, HolidayRegion, ImportantDate};
use crate::app::SignIn;
use pip_core::connectors::{self, Connector, ConnectorId};
use pip_core::settings::{Attention, Character, CustomReminder, EventAlerts, Liveliness, PetColor, PetSize, Theme, Voice};
use pip_core::time::{DaySet, TimeOfDay};
use tiny_skia::{PixmapMut, Transform};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const LOGICAL_W: i32 = 760;
pub const LOGICAL_H: i32 = 580;
const SIDEBAR_W: f32 = 196.0;
const HEADER_H: f32 = 92.0;
pub const T_USAGE: usize = 1;

pub fn page_at(i: usize) -> Page {
    PAGES[i.min(PAGES.len() - 1)].0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Pet,
    Reminders,
    Calendars,
    Email,
    About,
}

/// Water lives with the reminders because it is one, and dates live with the calendar because
/// that is what they are. Fewer pages, nothing hidden.
///
/// Connections is gone (§71). It was a page of its own that held the Gmail card, the calendar
/// card and the phone card — and the Gmail card was literally the same function the Email page
/// already drew, so the one thing on it anybody needed twice was the one thing it duplicated.
/// Each connector now sits on the page about the thing it connects, where you are already
/// standing when you want it, and the phone sits with About, which is the page about this PC.
const PAGES: [(Page, &str, &str); 5] = [
    (Page::Pet, "Companion", "Name, look and behaviour"),
    (Page::Reminders, "Reminders", "Water, your reminders, and when it speaks up"),
    (Page::Calendars, "Calendar", "Dates, holidays and your calendars"),
    (Page::Email, "Email", "A gentle nudge for mail that matters"),
    (Page::About, "About", "This PC, your phone and privacy"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Title(String),
    DateTitle(String),
    CalName(String),
    CalUrl(String),
    Keywords,
    MailSenders,
    MailKeywords,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiAct {
    Page(Page),
    Focus(Field),
    Color(PetColor),
    Size(PetSize),
    Live(Liveliness),
    Attn(Attention),
    Theme(Theme),
    Top,
    Clock,
    Startup,
    WaterOn,
    WaterEvery(i32),
    WaterStart(i32),
    WaterEnd(i32),
    WaterDay(u32),
    WaterGoal(i32),
    QuietOn,
    QuietStart(i32),
    QuietEnd(i32),
    Sound,
    AddReminder,
    RemOn(String),
    /// Nudge one of a reminder's times: id, which time, minutes.
    RemTime(String, usize, i32),
    RemTimeAdd(String),
    RemTimeRemove(String, usize),
    RemDay(String, u32),
    RemImportant(String),
    RemDelete(String),
    OpenData,
    Install,
    Uninstall,
    ResetPos,
    TestReminder,
    Holidays(HolidayRegion),
    MorningSummary,
    NotifyAt(i32),
    AddDate,
    DateKindSet(String, DateKind),
    DateMonth(String, i32),
    DateDay(String, i32),
    DateYear(String, i32),
    DateLead(String, u32),
    DateDelete(String),
    Alerts(EventAlerts),
    CalOn(String),
    CalImportant(String),
    CalDelete(String),
    SyncNow,
    GmailSignIn,
    GmailCancel,
    MailCheckNow,
    MailDisconnect(String),
    AddMailAccount,
    GmailReconnect(String),
    GmailReopen,
    /// None = connect; Some("") = add another account; Some(address) = reconnect that one.
    ConnectGoogleCalendar(Option<String>),
    SyncConnect,
    PhoneSyncNow,
    SyncDisconnect,
    /// "Yes, that's mine" / "Not now" on a device waiting to be let in.
    SyncApprove(String),
    SyncDeny(String),
    SyncAskAgain,
    Who(Character),
    Say(Voice),
}

pub struct SettingsUi {
    pub hwnd: HWND,
    surface: Surface,
    pub page: Page,
    hits: Vec<(R, UiAct)>,
    hover: Option<R>,
    pub focus: Option<Field>,
    /// A reminder added in this session and not yet named.
    ///
    /// It is created switched off, so that clicking "+ Add reminder" to see what happens cannot
    /// leave a live alarm behind, and it switches itself on the moment it is given a name. Held by
    /// id so that only this one is affected: turning on a reminder the person had deliberately
    /// switched off, just because they edited its title, would be its own small betrayal.
    just_added: Option<String>,

    /// What the keyboard is on, held as the action itself rather than as an index.
    ///
    /// Public so devtools can read it back: keyboard reach is the one thing here that cannot be
    /// checked by looking at a screenshot alone.
    ///
    /// `hits` is rebuilt from scratch every frame and the lists on these pages grow and shrink, so
    /// an index would mean that adding a reminder silently moved the focus to a different control.
    /// The action is what the person is actually pointing at, and it survives the rebuild.
    pub kb: Option<UiAct>,
    scroll: f32,
    max_scroll: f32,
    usage: Usage,
    cpu_rate: Option<f32>,
    usage_prev: Option<(u64, u64)>,
    /// Settings as they were when the current text edit began (diffed on commit).
    edit_origin: Option<Box<pip_core::Settings>>,
    /// Comma-separated keywords while that field is being edited.
    keywords_buf: String,
}

impl SettingsUi {
    pub fn new(hwnd: HWND) -> SettingsUi {
        SettingsUi {
            hwnd,
            surface: Surface::empty(),
            page: Page::Pet,
            hits: Vec::new(),
            hover: None,
            focus: None,
            just_added: None,
            kb: None,
            scroll: 0.0,
            max_scroll: 0.0,
            usage: platform::process_usage(),
            cpu_rate: None,
            usage_prev: None,
            edit_origin: None,
            keywords_buf: String::new(),
        }
    }

    pub fn scale(&self) -> f32 {
        unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(self.hwnd) as f32 / 96.0 }.max(1.0)
    }

    #[cfg(feature = "devtools")]
    pub fn snapshot_rgba(&mut self) -> (Vec<u8>, u32, u32) {
        (self.surface.rgba_copy(), self.surface.w as u32, self.surface.h as u32)
    }

    fn hit(&self, x: f32, y: f32) -> Option<&UiAct> {
        self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, a)| a)
    }
}

fn fmt_minutes(m: u32) -> String {
    match (m / 60, m % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m}"),
    }
}

/// Drawing context for one frame.
struct Ctx<'a, 'b> {
    pm: &'a mut PixmapMut<'b>,
    pal: Palette,
    s: f32,
    x0: f32,
    x1: f32,
    y: f32,
    hits: Vec<(R, UiAct)>,
    hover: Option<R>,
}

impl Ctx<'_, '_> {
    /// A labelled row; returns the vertical centre for the control on the right.
    fn row(&mut self, label: &str, hint: Option<&str>) -> f32 {
        let s = self.s;
        let h = if hint.is_some() { 60.0 * s } else { 52.0 * s };
        let f = fonts();
        let ls = 13.5 * s;
        if let Some(hint) = hint {
            text::draw(self.pm, &f.semibold, ls, self.x0, self.y + h / 2.0 - 3.0 * s, self.pal.text, 1.0, label);
            text::draw(self.pm, &f.regular, 12.0 * s, self.x0, self.y + h / 2.0 + 14.0 * s, self.pal.muted, 1.0, hint);
        } else {
            text::draw(self.pm, &f.semibold, ls, self.x0, ui::baseline(self.y, h, ls), self.pal.text, 1.0, label);
        }
        let cy = self.y + h / 2.0;
        self.y += h;
        fill_rrect(self.pm, self.x0, self.y - 1.0, self.x1 - self.x0, 1.0, 0.0, color(self.pal.border, 0.7));
        cy
    }

    fn toggle(&mut self, cy: f32, on: bool, act: UiAct) {
        let s = self.s;
        let r = ui::toggle(self.pm, self.x1 - ui::TOGGLE_W * s, cy - 11.0 * s, on, &self.pal, s);
        self.hits.push((r, act));
    }

    fn stepper(&mut self, cy: f32, x_right: f32, w: f32, value: &str, minus: UiAct, plus: UiAct) {
        let s = self.s;
        let (m, p) = ui::stepper(self.pm, x_right - w * s, cy - 15.0 * s, w * s, value, &self.pal, s);
        self.hits.push((m, minus));
        self.hits.push((p, plus));
    }

    fn segmented(&mut self, cy: f32, options: &[&str], selected: usize, acts: Vec<UiAct>) {
        let s = self.s;
        let w = ui::segmented_width(options, s);
        let rects = ui::segmented(self.pm, self.x1 - w, cy - 15.0 * s, options, selected, &self.pal, s);
        for (r, a) in rects.into_iter().zip(acts) {
            self.hits.push((r, a));
        }
    }

    fn button(&mut self, x: f32, y: f32, label: &str, style: BtnStyle, act: UiAct) -> f32 {
        let s = self.s;
        let w = ui::button_width(label, s);
        let r = R::new(x, y, w, 30.0 * s);
        let hover = self.hover == Some(r);
        ui::button(self.pm, r, label, style, &self.pal, s, hover);
        self.hits.push((r, act));
        w
    }
}

impl App {
    pub fn ui_opened(&mut self, hwnd: HWND) {
        let mut ui = SettingsUi::new(hwnd);
        if let Some(p) = self.open_page.take() {
            ui.page = page_at(p);
        }
        let about = ui.page == Page::About;
        self.ui = Some(ui);
        if about {
            self.sync_peek();
        }
    }

    pub fn ui_destroyed(&mut self) {
        self.ui = None; // frees the surface and everything else
        self.render();
        self.set_timer(crate::app::T_TRIM, 5_000);
    }

    pub fn ui_invalidate(&self) {
        if let Some(ui) = &self.ui {
            unsafe { InvalidateRect(ui.hwnd, std::ptr::null(), 0) };
        }
    }

    pub fn ui_page(&mut self, index: usize) {
        if let Some(ui) = &mut self.ui {
            ui.page = PAGES[index.min(PAGES.len() - 1)].0;
            ui.scroll = 0.0;
            ui.focus = None;
        }
        if PAGES[index.min(PAGES.len() - 1)].0 == Page::About {
            self.sync_peek();
        }
        self.ui_invalidate();
        self.ui_arm_usage();
    }

    fn ui_arm_usage(&self) {
        if let Some(ui) = &self.ui {
            unsafe {
                if ui.page == Page::About {
                    SetTimer(ui.hwnd, T_USAGE, 2_000, None);
                } else {
                    KillTimer(ui.hwnd, T_USAGE);
                }
            }
        }
    }

    pub fn ui_usage_tick(&mut self) {
        if let Some(ui) = &mut self.ui {
            let u = platform::process_usage();
            let now = tick_ms();
            if let Some((cpu, t)) = ui.usage_prev {
                let dt = now.saturating_sub(t).max(1) as f32;
                ui.cpu_rate = Some((u.cpu_ms.saturating_sub(cpu)) as f32 / dt * 100.0);
            }
            ui.usage_prev = Some((u.cpu_ms, now));
            ui.usage = u;
        }
        self.ui_invalidate();
    }

    pub fn ui_paint(&mut self) {
        let Some(mut ui) = self.ui.take() else { return };
        unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(ui.hwnd, &mut ps);
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(ui.hwnd, &mut rc);
            if ui.surface.ensure(rc.right - rc.left, rc.bottom - rc.top) {
                self.ui_draw(&mut ui);
                if ui.scroll > ui.max_scroll {
                    // Scroll extent is only known after layout; clamp and draw again.
                    ui.scroll = ui.max_scroll;
                    self.ui_draw(&mut ui);
                }
                ui.surface.blit(hdc);
            }
            EndPaint(ui.hwnd, &ps);
        }
        self.ui = Some(ui);
    }

    fn ui_draw(&self, ui: &mut SettingsUi) {
        let s = ui.scale();
        let pal = self.palette();
        let (w, h) = (ui.surface.w as f32, ui.surface.h as f32);
        let hover = ui.hover;
        let page = ui.page;
        let scroll = ui.scroll;
        let focus = ui.focus.clone();
        let usage = ui.usage;
        let cpu_rate = ui.cpu_rate;
        let kw_buf = ui.keywords_buf.clone();
        let Some(mut pm) = ui.surface.canvas() else { return };
        fill_rrect(&mut pm, 0.0, 0.0, w, h, 0.0, color(pal.window, 1.0));

        let x0 = (SIDEBAR_W + 36.0) * s;
        let x1 = w - 36.0 * s;
        let mut c = Ctx { pm: &mut pm, pal, s, x0, x1, y: HEADER_H * s - scroll, hits: Vec::new(), hover };

        match page {
            Page::Pet => self.page_pet(&mut c, &focus),

            Page::Reminders => self.page_reminders(&mut c, &focus),
            Page::Calendars => self.page_calendars(&mut c, &focus, &kw_buf),
            Page::Email => self.page_email(&mut c, &focus, &kw_buf),
            Page::About => self.page_about(&mut c, usage, cpu_rate),
        }
        let content_bottom = c.y + scroll + 28.0 * s;
        let mut hits: Vec<(R, UiAct)> = c.hits.into_iter().filter(|(r, _)| r.y + r.h > HEADER_H * s).collect();

        // Header (drawn over scrolled content)
        fill_rrect(&mut pm, SIDEBAR_W * s, 0.0, w, HEADER_H * s, 0.0, color(pal.window, 1.0));
        let (title, sub) = PAGES.iter().find(|p| p.0 == page).map(|p| (p.1, p.2)).unwrap_or(("", ""));
        let f = fonts();
        text::draw(&mut pm, &f.semibold, 22.0 * s, x0, 44.0 * s, pal.text, 1.0, title);
        text::draw(&mut pm, &f.regular, 12.5 * s, x0, 66.0 * s, pal.muted, 1.0, sub);
        let add = match page {
            Page::Reminders if self.settings.reminders.len() < 50 => Some(("+ Add reminder", UiAct::AddReminder)),
            Page::Calendars if self.settings.dates.items.len() < 200 => Some(("+ Add date", UiAct::AddDate)),
            _ => None,
        };
        if let Some((label, act)) = add {
            let bw = ui::button_width(label, s);
            let r = R::new(x1 - bw, 32.0 * s, bw, 30.0 * s);
            ui::button(&mut pm, r, label, BtnStyle::Primary, &pal, s, hover == Some(r));
            hits.push((r, act));
        }

        // Sidebar
        let side = if pal.dark { 0x171A20 } else { 0xEDEFF3 };
        fill_rrect(&mut pm, 0.0, 0.0, SIDEBAR_W * s, h, 0.0, color(side, 1.0));
        let cs = 0.62 * s;
        let base = Transform::from_translate((SIDEBAR_W * s - 100.0 * cs) / 2.0, 14.0 * s).pre_scale(cs, cs);
        let mut pose = Pose::base(Expr::Happy);
        pose.blush = 0.6;
        // The portrait is this companion, so it is as grown as it actually is.
        pose.stage = self.state.stage();
        creature::draw(&mut pm, &pose, pal.body, self.settings.pet.character.shown(self.state.experience), base);
        let name = &self.settings.pet.name;
        text::draw_centered(&mut pm, &f.semibold, 16.0 * s, SIDEBAR_W * s / 2.0, 100.0 * s, pal.text, 1.0, name);
        text::draw_centered(&mut pm, &f.regular, 11.5 * s, SIDEBAR_W * s / 2.0, 118.0 * s, pal.muted, 1.0, "your tiny friend");
        let mut ny = 142.0 * s;
        for (p, label, _) in PAGES {
            let r = R::new(12.0 * s, ny, (SIDEBAR_W - 24.0) * s, 36.0 * s);
            let selected = p == page;
            if selected {
                fill_rrect(&mut pm, r.x, r.y, r.w, r.h, 10.0 * s, color(pal.surface, 1.0));
                fill_rrect(&mut pm, r.x + 6.0 * s, r.y + 10.0 * s, 3.0 * s, r.h - 20.0 * s, 1.5 * s, color(pal.accent, 1.0));
            } else if hover == Some(r) {
                fill_rrect(&mut pm, r.x, r.y, r.w, r.h, 10.0 * s, color(pal.surface, 0.5));
            }
            let fs = 13.5 * s;
            let font = if selected { &f.semibold } else { &f.regular };
            text::draw(&mut pm, font, fs, r.x + 18.0 * s, ui::baseline(r.y, r.h, fs), if selected { pal.text } else { pal.muted }, 1.0, label);
            hits.push((r, UiAct::Page(p)));
            ny += 40.0 * s;
        }
        text::draw_centered(&mut pm, &f.regular, 11.0 * s, SIDEBAR_W * s / 2.0, h - 16.0 * s, pal.muted, 0.8, concat!("Pip ", env!("CARGO_PKG_VERSION")));

        // The focus ring, last, so it sits over whatever it is marking — including the sidebar and
        // the header, which are themselves painted over the scrolled content. Drawing it from the
        // hit list rather than inside each widget means every control gets one for free, including
        // the ones that do not know they are focusable.
        if let Some(act) = &ui.kb {
            if let Some((r, _)) = hits.iter().find(|(_, a)| a == act) {
                let pad = 3.0 * s;
                let (x, y, w2, h2) = (r.x - pad, r.y - pad, r.w + pad * 2.0, r.h + pad * 2.0);
                // Two strokes, not one: a single accent line disappears against the accent-coloured
                // controls it has to mark. The pale outer line keeps it visible on any of them,
                // and the pair reads as deliberate rather than as hover, which is a fill.
                stroke_rrect(&mut pm, x, y, w2, h2, 10.0 * s, color(pal.window, 0.9), 4.0 * s);
                stroke_rrect(&mut pm, x, y, w2, h2, 10.0 * s, color(pal.accent, 1.0), 2.0 * s);
            }
        }

        ui.hits = hits;
        ui.max_scroll = (content_bottom - h).max(0.0);
    }

    /// Three groups rather than one list of twelve (§71): who the companion is, how it behaves,
    /// and this window. Nothing moved pages and nothing was dropped — the labels are there so the
    /// eye can skip the two thirds of the page it did not come for. "Growing up" went to About,
    /// because it was never a setting: it is something earned, and it sits with the other things
    /// that are only true about this companion.
    fn page_pet(&self, c: &mut Ctx, focus: &Option<Field>) {
        let s = c.s;
        let p = &self.settings.pet;
        section_label(c, "WHO IT IS");
        let cy = c.row("Name", Some("What your companion is called"));
        let r = R::new(c.x1 - 220.0 * s, cy - 16.0 * s, 220.0 * s, 32.0 * s);
        ui::text_field(c.pm, r, &p.name, *focus == Some(Field::Name), &c.pal, s, "Pip");
        c.hits.push((r, UiAct::Focus(Field::Name)));

        // More characters than fit on one line: a single control is sized by its labels, so it
        // would have run off the left and over the description. Four to a line, each line its own
        // control right-aligned like every other one. A line that does not hold the current
        // character shows nothing selected, which is what an out-of-range index already does.
        // Only what has arrived is drawn. There is no locked state here and there is not going to
        // be one: a row of greyed-out creatures with the days left on them is a countdown, and the
        // pet does not do that (§7, §28, §43).
        let here: Vec<Character> = Character::available(self.state.experience).collect();
        let mut cy = c.row("Character", Some("Who your companion is — the rest stays the same"));
        for (i, chunk) in here.chunks(4).enumerate() {
            if i > 0 {
                cy = c.row("", None);
            }
            let labels: Vec<&str> = chunk.iter().map(|x| x.label()).collect();
            let sel = chunk.iter().position(|x| *x == p.character).unwrap_or(usize::MAX);
            c.segmented(cy, &labels, sel, chunk.iter().map(|x| UiAct::Who(*x)).collect());
        }

        let cy = c.row("Voice", Some("Each companion has its own; tap to hear"));
        let sel = Voice::ALL.iter().position(|x| *x == p.voice).unwrap_or(0);
        let labels: Vec<&str> = Voice::ALL.iter().map(|x| x.label()).collect();
        c.segmented(cy, &labels, sel, Voice::ALL.iter().map(|x| UiAct::Say(*x)).collect());

        let cy = c.row("Colour", None);
        let n = PetColor::ALL.len() as f32;
        for (i, col) in PetColor::ALL.iter().enumerate() {
            let cx = c.x1 - (n - i as f32) * 32.0 * s + 12.0 * s;
            let r = ui::swatch(c.pm, cx, cy, col.rgb(), *col == p.color, &c.pal, s);
            c.hits.push((r, UiAct::Color(*col)));
        }
        let cy = c.row("Size", None);
        let sel = match p.size { PetSize::Small => 0, PetSize::Medium => 1, PetSize::Large => 2 };
        c.segmented(cy, &["Small", "Medium", "Large"], sel, vec![UiAct::Size(PetSize::Small), UiAct::Size(PetSize::Medium), UiAct::Size(PetSize::Large)]);

        section_label(c, "HOW IT BEHAVES");
        let cy = c.row("Liveliness", Some("How often it stretches, fidgets and looks around"));
        let sel = match p.liveliness { Liveliness::Calm => 0, Liveliness::Normal => 1, Liveliness::Lively => 2 };
        c.segmented(cy, &["Calm", "Normal", "Lively"], sel, vec![UiAct::Live(Liveliness::Calm), UiAct::Live(Liveliness::Normal), UiAct::Live(Liveliness::Lively)]);
        let cy = c.row("Getting your attention", Some("How hard it tries when a reminder is due"));
        let sel = Attention::ALL.iter().position(|a| *a == p.attention).unwrap_or(2);
        c.segmented(
            cy,
            &Attention::ALL.map(|a| a.label()),
            sel,
            Attention::ALL.iter().map(|a| UiAct::Attn(*a)).collect(),
        );

        section_label(c, "ON YOUR DESKTOP");
        let cy = c.row("Theme", Some("Speech bubbles and this window"));
        let sel = match p.theme { Theme::System => 0, Theme::Light => 1, Theme::Dark => 2 };
        c.segmented(cy, &["System", "Light", "Dark"], sel, vec![UiAct::Theme(Theme::System), UiAct::Theme(Theme::Light), UiAct::Theme(Theme::Dark)]);
        let cy = c.row("Always on top", Some("Stay above other windows"));
        c.toggle(cy, p.always_on_top, UiAct::Top);
        let cy = c.row("Show clock", Some("A tiny clock above your pet, updated once a minute"));
        c.toggle(cy, p.show_clock, UiAct::Clock);
        let cy = c.row("Start with Windows", Some("Wake up quietly when you sign in"));
        c.toggle(cy, p.start_with_system, UiAct::Startup);
    }

    /// Water is a reminder Pip already knows how to time, so it sits with the rest of them.
    fn water_section(&self, c: &mut Ctx) {
        let w = &self.settings.water;
        let cy = c.row("Water reminders", Some("Gentle nudges, counted from your last glass"));
        c.toggle(cy, w.enabled, UiAct::WaterOn);
        let x1 = c.x1;
        let cy = c.row("Remind me every", None);
        c.stepper(cy, x1, 150.0, &fmt_minutes(w.interval_min), UiAct::WaterEvery(-15), UiAct::WaterEvery(15));
        // One row, two steppers: "Starting at" and "Until" were never two decisions, they were two
        // halves of one, and splitting them cost a whole row to say "Until" (§71).
        let cy = c.row("Between", Some("No water reminders outside these hours"));
        c.stepper(cy, x1 - 158.0 * c.s, 150.0, &w.start.to_string(), UiAct::WaterStart(-30), UiAct::WaterStart(30));
        c.stepper(cy, x1, 150.0, &w.end.to_string(), UiAct::WaterEnd(-30), UiAct::WaterEnd(30));
        let cy = c.row("On these days", None);
        let rects = ui::day_chips(c.pm, c.x1 - ui::DAY_CHIPS_W * c.s, cy - 13.0 * c.s, w.days.0, &c.pal, c.s);
        for (i, r) in rects.into_iter().enumerate() {
            c.hits.push((r, UiAct::WaterDay(i as u32)));
        }
        // What you have drunk today reads back under the goal rather than on a row of its own. It
        // was never something you could set, and About already says it in full.
        let cy = c.row("Daily goal", Some(&water_today(self)));
        let goal = if w.daily_goal == 1 { "1 glass".to_string() } else { format!("{} glasses", w.daily_goal) };
        c.stepper(cy, x1, 150.0, &goal, UiAct::WaterGoal(-1), UiAct::WaterGoal(1));
    }

    fn page_reminders(&self, c: &mut Ctx, focus: &Option<Field>) {
        let s = c.s;
        let f = fonts();
        section_label(c, "WATER");
        self.water_section(c);
        section_label(c, "YOUR REMINDERS");
        c.y += 6.0 * s;
        // The empty state ends its own section and nothing else. It used to `return`, which took
        // "When it speaks up" down with it — so on a fresh install, when nobody has added a
        // reminder yet, Do Not Disturb and Sound could not be reached at all. §21 fixed exactly
        // this on the Dates page and it had grown back here.
        if self.settings.reminders.is_empty() {
            c.y += 40.0 * s;
            let msg = "No reminders yet.";
            text::draw(c.pm, &f.semibold, 15.0 * s, c.x0, c.y, c.pal.text, 1.0, msg);
            c.y += 24.0 * s;
            for line in text::wrap(&f.regular, 13.0 * s, "Add one for stand-ups, medication, stretching, calling someone — anything with a time. Mark it important and it can gently break through do not disturb.", c.x1 - c.x0) {
                text::draw(c.pm, &f.regular, 13.0 * s, c.x0, c.y, c.pal.muted, 1.0, &line);
                c.y += 19.0 * s;
            }
            section_label(c, "WHEN IT SPEAKS UP");
            self.quiet_section(c);
            return;
        }
        let now = platform::now_unix();
        for r in &self.settings.reminders {
            // Times wrap onto as many rows as they need, so "morning, noon and night" is one card
            // rather than three reminders.
            let cell_w = 122.0 * s + if r.times.len() > 1 { 24.0 * s } else { 0.0 };
            let per_row = (((c.x1 - c.x0) / cell_w).floor() as usize).max(1);
            let cells = r.times.len() + usize::from(r.times.len() < 12); // + the "add" button
            let time_rows = cells.div_ceil(per_row).max(1);
            let card_h = (52.0 + time_rows as f32 * 34.0 + 34.0 + 34.0 + 12.0) * s;
            let (x, y, w) = (c.x0 - 14.0 * s, c.y + 6.0 * s, c.x1 - c.x0 + 28.0 * s);
            fill_rrect(c.pm, x, y, w, card_h - 12.0 * s, 14.0 * s, color(c.pal.surface, 1.0));
            if r.important {
                fill_rrect(c.pm, x + 5.0 * s, y + 16.0 * s, 3.5 * s, card_h - 44.0 * s, 2.0 * s, color(0xFFB020, 1.0));
            }
            // Row 1: title + enabled
            let tr = R::new(c.x0, y + 12.0 * s, c.x1 - c.x0 - 60.0 * s, 32.0 * s);
            ui::text_field(c.pm, tr, &r.title, *focus == Some(Field::Title(r.id.clone())), &c.pal, s, "What to remind you about, e.g. take vitamins");
            c.hits.push((tr, UiAct::Focus(Field::Title(r.id.clone()))));
            let tg = ui::toggle(c.pm, c.x1 - ui::TOGGLE_W * s, y + 17.0 * s, r.enabled, &c.pal, s);
            c.hits.push((tg, UiAct::RemOn(r.id.clone())));
            // Row 2: every time of day it goes off, then the days it repeats on
            let mut tx = c.x0;
            let mut ty = y + 52.0 * s;
            for (i, t) in r.times.iter().enumerate() {
                if tx + 118.0 * s > c.x1 {
                    tx = c.x0;
                    ty += 34.0 * s;
                }
                let (m, p) = ui::stepper(c.pm, tx, ty, 118.0 * s, &t.to_string(), &c.pal, s);
                c.hits.push((m, UiAct::RemTime(r.id.clone(), i, -15)));
                c.hits.push((p, UiAct::RemTime(r.id.clone(), i, 15)));
                tx += 122.0 * s;
                // A reminder always keeps one time, so the last one has nothing to remove.
                if r.times.len() > 1 {
                    let rm = R::new(tx - 4.0 * s, ty, 22.0 * s, 30.0 * s);
                    let rgb = if c.hover == Some(rm) { c.pal.text } else { c.pal.muted };
                    text::draw_centered(c.pm, &f.semibold, 15.0 * s, rm.x + 11.0 * s, ty + 20.0 * s, rgb, 1.0, "×");
                    c.hits.push((rm, UiAct::RemTimeRemove(r.id.clone(), i)));
                    tx += 24.0 * s;
                }
            }
            if r.times.len() < 12 {
                let bw = ui::button_width("+ time", s);
                if tx + bw > c.x1 {
                    tx = c.x0;
                    ty += 34.0 * s;
                }
                let br = R::new(tx, ty + 1.0 * s, bw, 28.0 * s);
                ui::button(c.pm, br, "+ time", BtnStyle::Ghost, &c.pal, s, c.hover == Some(br));
                c.hits.push((br, UiAct::RemTimeAdd(r.id.clone())));
            }
            let days_y = ty + 36.0 * s;
            let chips = ui::day_chips(c.pm, c.x0, days_y, r.days.0, &c.pal, s);
            for (i, cr) in chips.into_iter().enumerate() {
                c.hits.push((cr, UiAct::RemDay(r.id.clone(), i as u32)));
            }
            // Row 3: next occurrence + actions
            let row3 = days_y + 32.0 * s;
            let next = if !r.enabled {
                "Off".to_string()
            } else {
                match schedule::reminder_next(r, &self.state, now, &chrono::Local) {
                    Some(t) => {
                        let l = pip_core::time::local(&chrono::Local, t);
                        let today = pip_core::time::local(&chrono::Local, now).date();
                        let day = if l.date() == today {
                            "today".to_string()
                        } else if Some(l.date()) == today.succ_opt() {
                            "tomorrow".to_string()
                        } else {
                            pip_core::time::weekday_name(chrono::Datelike::weekday(&l.date())).to_string()
                        };
                        let rep = if r.days.is_empty() { "Once".to_string() } else { r.days.describe() };
                        format!("{rep}  ·  next {day} at {}", hm(t))
                    }
                    None => "Not scheduled".into(),
                }
            };
            text::draw(c.pm, &f.regular, 12.0 * s, c.x0, row3 + 19.0 * s, c.pal.muted, 1.0, &next);
            let del_w = ui::button_width("Delete", s);
            let imp_label = if r.important { "Important  ✓" } else { "Mark important" };
            let imp_w = ui::button_width(imp_label, s);
            let rx = c.x1 - del_w;
            let dr = R::new(rx, row3, del_w, 28.0 * s);
            ui::button(c.pm, dr, "Delete", BtnStyle::Danger, &c.pal, s, c.hover == Some(dr));
            c.hits.push((dr, UiAct::RemDelete(r.id.clone())));
            let ir = R::new(rx - imp_w - 6.0 * s, row3, imp_w, 28.0 * s);
            ui::button(c.pm, ir, imp_label, if r.important { BtnStyle::Primary } else { BtnStyle::Secondary }, &c.pal, s, c.hover == Some(ir));
            c.hits.push((ir, UiAct::RemImportant(r.id.clone())));
            c.y += card_h;
        }
        section_label(c, "WHEN IT SPEAKS UP");
        self.quiet_section(c);
    }

    /// Quiet hours and how reminders arrive. A section of the Reminders page rather than a page:
    /// four of its seven rows were never about quiet hours at all — sound, snooze, how long a
    /// bubble waits, pausing for fullscreen — and all seven answer the same question as the page
    /// they now live on, which is when and how you get told things.
    ///
    /// Four of those rows are gone (§71), and the two steppers that are left share a line. What
    /// went was not functionality: snooze is still ten minutes, the bubble still folds away after
    /// three, reminders still wait for a fullscreen game, and an important one still arrives
    /// silently while you are not being disturbed. `Settings::sanitize` pins all four, so the only
    /// thing removed is being asked.
    fn quiet_section(&self, c: &mut Ctx) {
        let q = &self.settings.quiet;
        let n = &self.settings.notify;
        let x1 = c.x1;
        let cy = c.row("Do not disturb", Some("Reminders wait, except important ones. Your companion sleeps either way"));
        c.toggle(cy, q.enabled, UiAct::QuietOn);
        let cy = c.row("Between", None);
        c.stepper(cy, x1 - 158.0 * c.s, 150.0, &q.start.to_string(), UiAct::QuietStart(-30), UiAct::QuietStart(30));
        c.stepper(cy, x1, 150.0, &q.end.to_string(), UiAct::QuietEnd(-30), UiAct::QuietEnd(30));
        let cy = c.row("Sound", Some("Pip has a voice; this is whether you hear it"));
        c.toggle(cy, n.sound, UiAct::Sound);
    }

    fn page_about(&self, c: &mut Ctx, u: Usage, cpu_rate: Option<f32>) {
        let s = c.s;
        let f = fonts();
        let now = platform::now_unix();
        let days = ((now - self.state.created_at).max(0) / 86_400) as u32;
        let (x, w) = (c.x0 - 14.0 * s, c.x1 - c.x0 + 28.0 * s);

        // Stats card. "Growing up" came here off the Companion page (§71): it was a read-only row
        // among eleven controls, which is the one place a fact does not belong.
        let y = c.y + 4.0 * s;
        fill_rrect(c.pm, x, y, w, 98.0 * s, 14.0 * s, color(c.pal.surface, 1.0));
        let with = match days {
            0 => "arrived today".to_string(),
            1 => "has been with you for 1 day".to_string(),
            d => format!("has been with you for {d} days"),
        };
        text::draw(c.pm, &f.semibold, 15.0 * s, c.x0, y + 30.0 * s, c.pal.text, 1.0, &format!("{} {with}", self.settings.pet.name));
        text::draw(c.pm, &f.regular, 12.5 * s, c.x0, y + 54.0 * s, c.pal.muted, 1.0, &pip_core::messages::growth_label(self.state.experience));
        let stats = format!(
            "Today {} of {} glasses  ·  streak {} (best {})",
            local_count(self),
            self.settings.water.daily_goal,
            self.state.streak,
            self.state.best_streak
        );
        text::draw(c.pm, &f.regular, 12.5 * s, c.x0, y + 76.0 * s, c.pal.muted, 1.0, &stats);
        c.y = y + 114.0 * s;

        // Your phone, which came off the Connections page with the other two connectors (§71).
        // It belongs on the page about this PC rather than on a page of its own: it is the one
        // connector that is not about a service at all.
        section_label(c, "YOUR PHONE");
        self.draw_sync_connector(c);

        // Resource card
        let y = c.y;
        fill_rrect(c.pm, x, y, w, 128.0 * s, 14.0 * s, color(c.pal.surface, 1.0));
        text::draw(c.pm, &f.semibold, 13.5 * s, c.x0, y + 28.0 * s, c.pal.text, 1.0, "Resource use");
        let up_s = tick_ms().saturating_sub(self.started_ms) / 1000;
        let mb = |b: u64| b as f32 / (1024.0 * 1024.0);
        let lines = [
            format!("Memory  {:.1} MB private  ·  {:.1} MB working set", mb(u.private_bytes), mb(u.working_set)),
            format!(
                "CPU  {:.2} s total in {}h {:02}m{}",
                u.cpu_ms as f32 / 1000.0,
                up_s / 3600,
                (up_s / 60) % 60,
                cpu_rate.map(|r| format!("  ·  now {r:.2} %")).unwrap_or_default()
            ),
            format!("Startup {} ms  ·  1 process  ·  {} handles  ·  {} frames drawn", self.startup_ms, u.handles, self.pet.frames),
        ];
        for (i, l) in lines.iter().enumerate() {
            text::draw(c.pm, &f.regular, 12.5 * s, c.x0, y + (54.0 + 23.0 * i as f32) * s, c.pal.muted, 1.0, l);
        }
        c.y = y + 144.0 * s;

        // Privacy
        // The sidebar already carries the version, so this only has to say where the evidence is.
        let version_line = "If something ever goes wrong, what happened is written to pip.log in that folder.";
        // What leaves this PC depends on what has been connected, so the claim is built from what
        // is actually switched on. "No network access" was true in Phase 1 and has not been since
        // Phase 2, and a promise a stranger can catch out is worse than no promise at all.
        let gmail_on = !self.settings.email.accounts.is_empty();
        let cal_on =
            self.settings.dates.calendars.iter().any(|fd| pip_core::gcal::feed_address(&fd.url).is_some());
        let sync_on = self.sync_connected();
        let mut privacy =
            "Pip has no account of its own, no server behind it, and never tracks you. Your settings live in a small folder you can open below."
                .to_string();
        let mut told: Vec<&str> = Vec::new();
        if gmail_on {
            told.push("it reads who your mail is from, the subject and the labels, never the message itself");
        }
        if cal_on {
            told.push("it reads your calendar and never changes it");
        }
        if sync_on {
            told.push("your devices pass one encrypted file through your own Google Drive");
        }
        if !told.is_empty() {
            privacy.push_str(" Pip talks to Google only, signed in as you: ");
            privacy.push_str(&told.join("; "));
            privacy.push('.');
        }
        for l in text::wrap(&f.regular, 12.5 * s, &privacy, c.x1 - c.x0) {
            text::draw(c.pm, &f.regular, 12.5 * s, c.x0, c.y + 14.0 * s, c.pal.muted, 1.0, &l);
            c.y += 19.0 * s;
        }
        for l in text::wrap(&f.regular, 12.5 * s, version_line, c.x1 - c.x0) {
            text::draw(c.pm, &f.regular, 12.5 * s, c.x0, c.y + 14.0 * s, c.pal.muted, 1.0, &l);
            c.y += 19.0 * s;
        }
        c.y += 14.0 * s;
        let mut bx = c.x0;
        let y = c.y;
        bx += c.button(bx, y, "Open data folder", BtnStyle::Secondary, UiAct::OpenData) + 8.0 * s;
        bx += c.button(bx, y, "Reset position", BtnStyle::Secondary, UiAct::ResetPos) + 8.0 * s;
        c.button(bx, y, "Test a reminder", BtnStyle::Secondary, UiAct::TestReminder);
        c.y += 46.0 * s;

        // Where this copy of Pip lives, and how to install or remove it.
        let installed = crate::install::running_installed();
        let note = if installed {
            "Pip is installed on this PC and starts with Windows. You can remove it here or from Apps & features.".to_string()
        } else {
            format!("Pip is running from {}. Installing copies it to your account’s programs folder and adds it to the Start menu.", crate::install::current_folder())
        };
        for l in text::wrap(&f.regular, 12.5 * s, &note, c.x1 - c.x0) {
            text::draw(c.pm, &f.regular, 12.5 * s, c.x0, c.y + 14.0 * s, c.pal.muted, 1.0, &l);
            c.y += 19.0 * s;
        }
        c.y += 14.0 * s;
        if installed {
            c.button(c.x0, c.y, "Remove Pip…", BtnStyle::Danger, UiAct::Uninstall);
        } else {
            c.button(c.x0, c.y, "Install Pip on this PC", BtnStyle::Primary, UiAct::Install);
        }
        c.y += 40.0 * s;
    }

    // -----------------------------------------------------------------------------------------
    // Input
    // -----------------------------------------------------------------------------------------

    pub fn ui_mouse_move(&mut self, x: f32, y: f32) {
        let Some(ui) = &mut self.ui else { return };
        let hover = ui.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(r, _)| *r);
        if hover != ui.hover {
            ui.hover = hover;
            unsafe { InvalidateRect(ui.hwnd, std::ptr::null(), 0) };
        }
    }

    pub fn ui_cursor(&self, x: f32, y: f32) -> Option<*const u16> {
        let ui = self.ui.as_ref()?;
        match ui.hit(x, y)? {
            UiAct::Focus(_) => Some(IDC_IBEAM),
            _ => Some(IDC_HAND),
        }
    }

    pub fn ui_wheel(&mut self, delta: i32) {
        let Some(ui) = &mut self.ui else { return };
        let s = ui.scale();
        ui.scroll = (ui.scroll - delta as f32 / 120.0 * 48.0 * s).clamp(0.0, ui.max_scroll);
        unsafe { InvalidateRect(ui.hwnd, std::ptr::null(), 0) };
    }

    pub fn ui_click(&mut self, x: f32, y: f32) {
        let Some(ui) = &self.ui else { return };
        let act = ui.hit(x, y).cloned();
        // Clicking moves the keyboard with it, so Tab carries on from where the mouse left off.
        if let (Some(a), Some(ui)) = (act.clone(), self.ui.as_mut()) {
            ui.kb = Some(a);
        }
        let Some(ui) = &self.ui else { return };
        let was_focus = ui.focus.clone();
        // Leaving a text field (for anything but the same field) commits it.
        if was_focus.is_some() && act != was_focus.clone().map(UiAct::Focus) {
            if let Some(ui) = &mut self.ui {
                ui.focus = None;
            }
            self.ui_commit_text();
        }
        match act {
            Some(a) => self.ui_activate(a),
            None => {}
        }
        // Remember where an edit started (adds may have focused a new field).
        let snapshot = self.ui.as_ref().is_some_and(|u| u.focus.is_some() && u.edit_origin.is_none());
        if snapshot {
            let s = Box::new(self.settings.clone());
            if let Some(ui) = &mut self.ui {
                ui.edit_origin = Some(s);
            }
        }
        self.ui_invalidate();
    }

    /// Does one thing, however it was asked for. The click path and the keyboard both end here, so
    /// Enter on a control cannot drift from what clicking it does.
    fn ui_activate(&mut self, act: UiAct) {
        match act {
            UiAct::Focus(f) => {
                let list = match f {
                    Field::Keywords => Some(self.settings.dates.keywords.join(", ")),
                    Field::MailSenders => Some(self.settings.email.senders.join(", ")),
                    Field::MailKeywords => Some(self.settings.email.keywords.join(", ")),
                    _ => None,
                };
                if let Some(ui) = &mut self.ui {
                    if let (Some(list), true) = (list, ui.focus.as_ref() != Some(&f)) {
                        ui.keywords_buf = list;
                    }
                    ui.focus = Some(f);
                }
            }
            a => self.ui_do(a),
        }
        self.ui_invalidate();
    }

    /// Everything the keyboard can reach, in reading order.
    ///
    /// `hits` is stored in draw order, which puts the scrolling content first and the sidebar and
    /// header last, because those are painted over the top. Reading order is the other way round,
    /// so they are lifted to the front here rather than the drawing being rearranged to suit the
    /// keyboard.
    fn tab_order(&self) -> Vec<UiAct> {
        let Some(ui) = &self.ui else { return Vec::new() };
        let edge = SIDEBAR_W * ui.scale();
        let (side, body): (Vec<_>, Vec<_>) = ui.hits.iter().partition(|(r, _)| r.x < edge);
        let (header, content): (Vec<_>, Vec<_>) = body.into_iter().partition(|(r, _)| r.y + r.h <= HEADER_H * ui.scale());
        side.into_iter().chain(header).chain(content).map(|(_, a)| a.clone()).collect()
    }

    /// Scrolls the focused control into view, if it is off the top or the bottom.
    fn ui_reveal(&mut self, act: &UiAct) {
        let Some(ui) = &self.ui else { return };
        let s = ui.scale();
        let Some((r, _)) = ui.hits.iter().find(|(_, a)| a == act) else { return };
        let (top, bottom) = (HEADER_H * s, ui.surface.h as f32);
        let pad = 12.0 * s;
        let delta = if r.y - pad < top {
            r.y - pad - top
        } else if r.y + r.h + pad > bottom {
            r.y + r.h + pad - bottom
        } else {
            return;
        };
        if let Some(ui) = &mut self.ui {
            ui.scroll = (ui.scroll + delta).clamp(0.0, ui.max_scroll);
        }
    }

    /// Keys that work the window itself. Returns true when the key was used.
    ///
    /// Esc is deliberately not here: it stays in the window procedure, where it already leaves a
    /// text field first and closes the window second.
    pub fn ui_key(&mut self, vk: u16, shift: bool) -> bool {
        const VK_TAB: u16 = 0x09;
        const VK_RETURN: u16 = 0x0D;
        const VK_SPACE: u16 = 0x20;
        const VK_LEFT: u16 = 0x25;
        const VK_UP: u16 = 0x26;
        const VK_RIGHT: u16 = 0x27;
        const VK_DOWN: u16 = 0x28;
        if self.ui.is_none() {
            return false;
        }
        // While a text field is being edited the arrows and Space belong to the text, and Enter
        // ends the edit. Only Tab reaches past it, committing on the way out.
        let editing = self.ui.as_ref().is_some_and(|u| u.focus.is_some());
        if editing && !matches!(vk, VK_TAB | VK_RETURN) {
            return false;
        }
        let order = self.tab_order();
        if order.is_empty() {
            return false;
        }
        let current = self.ui.as_ref().and_then(|u| u.kb.clone());
        let at = current.as_ref().and_then(|a| order.iter().position(|x| x == a));

        let next = match vk {
            VK_TAB | VK_DOWN | VK_UP => {
                if editing {
                    if let Some(ui) = &mut self.ui {
                        ui.focus = None;
                    }
                    self.ui_commit_text();
                }
                let back = shift || vk == VK_UP;
                let n = order.len();
                Some(match at {
                    Some(i) if back => order[(i + n - 1) % n].clone(),
                    Some(i) => order[(i + 1) % n].clone(),
                    None => order[if back { n - 1 } else { 0 }].clone(),
                })
            }
            // Within one segmented control. Same variant, not same row: the characters are drawn
            // as two rows and must feel like the single control they are.
            VK_LEFT | VK_RIGHT => {
                let Some(cur) = current.clone() else { return false };
                let same: Vec<&UiAct> =
                    order.iter().filter(|a| std::mem::discriminant(*a) == std::mem::discriminant(&cur)).collect();
                let Some(i) = same.iter().position(|a| **a == cur) else { return false };
                let step = if vk == VK_LEFT { same.len() - 1 } else { 1 };
                let target = same[(i + step) % same.len()].clone();
                // Moving along a segmented control chooses as it goes, the way it behaves anywhere
                // else in Windows.
                if let Some(ui) = &mut self.ui {
                    ui.kb = Some(target.clone());
                }
                self.ui_reveal(&target);
                self.ui_activate(target);
                return true;
            }
            VK_RETURN | VK_SPACE => {
                if editing {
                    if let Some(ui) = &mut self.ui {
                        ui.focus = None;
                    }
                    self.ui_commit_text();
                    self.ui_invalidate();
                    return true;
                }
                let Some(act) = current else { return false };
                self.ui_activate(act);
                return true;
            }
            _ => None,
        };
        let Some(next) = next else { return false };
        if let Some(ui) = &mut self.ui {
            ui.kb = Some(next.clone());
        }
        self.ui_reveal(&next);
        self.ui_invalidate();
        true
    }

    fn ui_commit_text(&mut self) {
        // Diff against the settings as they were when editing began, so e.g. a changed calendar
        // link is noticed (and re-encrypted + synced) even though edits were applied live.
        let prev = self.ui.as_mut().and_then(|u| u.edit_origin.take()).map(|b| *b).unwrap_or_else(|| self.settings.clone());
        // A reminder just added is switched on by naming it. Giving something a name is the moment
        // it stops being an experiment, and it saves the person a second click they would resent.
        if let Some(id) = self.ui.as_mut().and_then(|u| u.just_added.take()) {
            if let Some(r) = self.settings.reminder_mut(&id) {
                if !r.title.trim().is_empty() {
                    r.enabled = true;
                }
            }
        }
        self.settings.sanitize();
        if prev != self.settings {
            self.settings_changed(&prev);
        }
        self.render();
    }

    pub fn ui_char(&mut self, ch: u32) {
        let Some(focus) = self.ui.as_ref().and_then(|u| u.focus.clone()) else { return };
        if matches!(ch, 0x0D | 0x1B | 0x09) {
            if let Some(ui) = &mut self.ui {
                ui.focus = None;
            }
            self.ui_commit_text();
            self.ui_invalidate();
            return;
        }
        // Ctrl+V pastes (single line; control characters dropped).
        let typed: String = match ch {
            0x16 => platform::clipboard_text().unwrap_or_default().chars().filter(|c| !c.is_control()).collect(),
            0x08 => String::new(),
            c if c >= 0x20 && c != 0x7F => char::from_u32(c).map(String::from).unwrap_or_default(),
            _ => return,
        };
        let backspace = ch == 0x08;
        let edit = |target: &mut String, max: usize| {
            if backspace {
                target.pop();
            }
            for c in typed.chars() {
                if target.chars().count() >= max {
                    break;
                }
                target.push(c);
            }
        };
        let now = platform::now_unix();
        match &focus {
            Field::Keywords | Field::MailSenders | Field::MailKeywords => {
                if let Some(ui) = &mut self.ui {
                    edit(&mut ui.keywords_buf, 500);
                    let list: Vec<String> = ui.keywords_buf.split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect();
                    match focus {
                        Field::MailSenders => self.settings.email.senders = list,
                        Field::MailKeywords => self.settings.email.keywords = list,
                        _ => self.settings.dates.keywords = list,
                    }
                }
            }
            Field::Name => edit(&mut self.settings.pet.name, 24),
            Field::Title(id) => {
                if let Some(r) = self.settings.reminders.iter_mut().find(|r| &r.id == id) {
                    r.updated_at = now;
                    edit(&mut r.title, 60);
                }
            }
            Field::DateTitle(id) => {
                if let Some(d) = self.settings.dates.items.iter_mut().find(|d| &d.id == id) {
                    d.updated_at = now;
                    edit(&mut d.title, 60);
                }
            }
            Field::CalName(id) => {
                if let Some(c) = self.settings.dates.calendars.iter_mut().find(|c| &c.id == id) {
                    c.updated_at = now;
                    edit(&mut c.name, 40);
                }
            }
            Field::CalUrl(id) => {
                if let Some(c) = self.settings.dates.calendars.iter_mut().find(|c| &c.id == id) {
                    c.updated_at = now;
                    edit(&mut c.url, 2000);
                }
            }
        }
        self.settings_dirty_changed();
        self.ui_invalidate();
    }

    /// Text edits: save and refresh, without the heavier rescheduling path (that runs on commit).
    fn settings_dirty_changed(&mut self) {
        self.schedule_settings_save();
        self.render();
        if let Some(ui) = &self.ui {
            let title = crate::platform::wide(&format!("{} — Settings", self.settings.pet.name));
            unsafe { SetWindowTextW(ui.hwnd, title.as_ptr()) };
        }
    }

    fn ui_do(&mut self, act: UiAct) {
        let prev = self.settings.clone();
        if self.ui_do_email(&act) {
            return;
        }
        if self.ui_do_dates(&act) {
            if prev != self.settings {
                self.settings_changed(&prev);
            }
            return;
        }
        let now = platform::now_unix();
        // Read before the settings are borrowed: the voice preview needs to know which companion
        // is actually on screen, and `shown` depends on how far it has grown.
        let xp = self.state.experience;
        let st = &mut self.settings;
        match act {
            UiAct::Page(p) => {
                if let Some(ui) = &mut self.ui {
                    ui.page = p;
                    ui.scroll = 0.0;
                }
                self.ui_arm_usage();
                if p == Page::About {
                    self.ui_usage_tick();
                    self.sync_peek();
                }
                return;
            }
            UiAct::Focus(_) => return,
            // Each of these stamps the appearance record. Without it the stamp stays at 0 forever
            // and any device that merely has a clock beats a choice actually made here (§48).
            UiAct::Who(w) => {
                st.pet.character = w;
                st.appearance_updated_at = platform::now_unix();
            }
            UiAct::Say(v) => {
                st.pet.voice = v;
                st.appearance_updated_at = platform::now_unix();
                // Choosing a voice plays it, whether or not reminder sounds are on: the tap is
                // the request, and a silent answer would look broken.
                //
                // A different phrase each tap, because the plain chime is now one of five things
                // this voice can say (§59) and a preview that only ever plays one of them is a
                // preview of the wrong thing. Tapping the same voice twice is meant to sound
                // slightly different; that is the point being demonstrated.
                //
                // These are the *things it says*, not the shapes: §66 gave each creature its own
                // phrasing, so a preview made of raw shapes would audition noises this companion
                // never makes. Run through the dialect, tapping Bunny plays Bunny's chime.
                const SAMPLER: [crate::attention::Sfx; 5] = [
                    crate::attention::Sfx::Voice,
                    crate::attention::Sfx::Nudge,
                    crate::attention::Sfx::Trill,
                    crate::attention::Sfx::Tap,
                    crate::attention::Sfx::Sigh,
                ];
                let n = platform::now_unix().unsigned_abs() as usize;
                let seed = n.wrapping_mul(6_364_136_223_846_793_005) as u64;
                // The preview must use the companion actually on screen, or "Its own" would audition
                // somebody else's voice.
                let who = st.pet.character.shown(xp);
                let ph = crate::attention::dialect(who, SAMPLER[n % SAMPLER.len()]);
                platform::Chime::say(v, who, ph, 1.0, seed).play();
            }
            UiAct::Color(c) => {
                st.pet.color = c;
                st.appearance_updated_at = platform::now_unix();
            }
            UiAct::Size(z) => {
                st.pet.size = z;
                st.appearance_updated_at = platform::now_unix();
            }
            UiAct::Live(l) => {
                st.pet.liveliness = l;
                st.appearance_updated_at = platform::now_unix();
            }
            UiAct::Attn(a) => {
                st.pet.attention = a;
                st.appearance_updated_at = platform::now_unix();
            }
            UiAct::Theme(t) => {
                st.pet.theme = t;
                st.appearance_updated_at = platform::now_unix();
            }
            UiAct::Top => st.pet.always_on_top = !st.pet.always_on_top,
            UiAct::Clock => st.pet.show_clock = !st.pet.show_clock,
            UiAct::Startup => st.pet.start_with_system = !st.pet.start_with_system,
            UiAct::WaterOn => st.water.enabled = !st.water.enabled,
            UiAct::WaterEvery(d) => st.water.interval_min = (st.water.interval_min as i32 + d).clamp(15, 240) as u32,
            UiAct::WaterStart(d) => st.water.start = st.water.start.add_minutes(d),
            UiAct::WaterEnd(d) => st.water.end = st.water.end.add_minutes(d),
            UiAct::WaterDay(i) => st.water.days.toggle(i),
            UiAct::WaterGoal(d) => st.water.daily_goal = (st.water.daily_goal as i32 + d).clamp(1, 20) as u32,
            UiAct::QuietOn => st.quiet.enabled = !st.quiet.enabled,
            UiAct::QuietStart(d) => st.quiet.start = st.quiet.start.add_minutes(d),
            UiAct::QuietEnd(d) => st.quiet.end = st.quiet.end.add_minutes(d),
            UiAct::Sound => st.notify.sound = !st.notify.sound,
            UiAct::AddReminder => {
                let next_hour = TimeOfDay::of(pip_core::time::local(&chrono::Local, now + 3600)).0 / 60 * 60;
                let id = format!("r{:x}{:04x}", now, (platform::tick_ms() & 0xffff));
                // Empty title, and switched off until it has one. Clicking Add to see what the
                // button does used to leave a nameless reminder that went off an hour later.
                let mut fresh = CustomReminder::new(id.clone(), "", vec![TimeOfDay(next_hour)], DaySet::WEEKDAYS, now);
                fresh.enabled = false;
                st.reminders.push(fresh);
                if let Some(ui) = &mut self.ui {
                    ui.just_added = Some(id.clone());
                    ui.focus = Some(Field::Title(id));
                    ui.scroll = f32::MAX; // clamped on next paint → shows the new card
                }
            }
            UiAct::RemOn(id) => {
                if let Some(r) = st.reminder_mut(&id) {
                    r.enabled = !r.enabled;
                    r.updated_at = now;
                    if r.enabled && r.days.is_empty() {
                        r.date = Some(schedule::next_date_for(r.first_time(), now, &chrono::Local));
                    }
                }
            }
            UiAct::RemTime(id, at, d) => {
                if let Some(r) = st.reminder_mut(&id) {
                    r.shift_time(at, d);
                    r.updated_at = now;
                    if r.days.is_empty() {
                        r.date = Some(schedule::next_date_for(r.first_time(), now, &chrono::Local));
                    }
                }
            }
            UiAct::RemTimeAdd(id) => {
                if let Some(r) = st.reminder_mut(&id) {
                    r.add_time();
                    r.updated_at = now;
                }
            }
            UiAct::RemTimeRemove(id, at) => {
                if let Some(r) = st.reminder_mut(&id) {
                    r.remove_time(at);
                    r.updated_at = now;
                    if r.days.is_empty() {
                        r.date = Some(schedule::next_date_for(r.first_time(), now, &chrono::Local));
                    }
                }
            }
            UiAct::RemDay(id, i) => {
                if let Some(r) = st.reminder_mut(&id) {
                    r.days.toggle(i);
                    r.updated_at = now;
                    // No days = a one-off at the next occurrence of its first time.
                    r.date = r.days.is_empty().then(|| schedule::next_date_for(r.first_time(), now, &chrono::Local));
                }
            }
            UiAct::RemImportant(id) => {
                if let Some(r) = st.reminder_mut(&id) {
                    r.important = !r.important;
                    r.updated_at = now;
                }
            }
            UiAct::RemDelete(id) => st.reminders.retain(|r| r.id != id),
            UiAct::Install => {
                self.effects.push(Effect::Install);
                return;
            }
            UiAct::Uninstall => {
                self.effects.push(Effect::Uninstall);
                return;
            }
            UiAct::OpenData => {
                platform::open_folder(self.store.dir());
                return;
            }
            UiAct::ResetPos => {
                self.pet.anchor = crate::pet::Pet::default_anchor(self.size());
                self.state.desktop_pos = None;
                if !self.pet.visible {
                    self.set_visible(true);
                }
                self.schedule_save();
                self.anim(crate::render::creature::AnimKind::Hop);
                return;
            }
            UiAct::TestReminder => {
                if !self.pet.visible {
                    self.set_visible(true);
                }
                let (t, b) = pip_core::messages::water_nudge(&self.settings.pet.name, self.state.next_msg(), 0, 8);
                let mut card = crate::pet::Card::new(crate::pet::BubbleKind::Message).line(b, false);
                card.title = Some(format!("Test  ·  {t}"));
                card.buttons.push(("Looks good".into(), crate::pet::Action::Close, BtnStyle::Primary));
                self.pet.show_bubble(card);
                self.set_timer(crate::app::T_BUBBLE, 15_000);
                self.anim(crate::render::creature::AnimKind::Wave);
                if self.settings.notify.sound {
                    let who = self.settings.pet.character.shown(self.state.experience);
                    platform::Chime::new(self.settings.pet.voice, who).play();
                }
                return;
            }
            _ => return, // dates/calendar actions are handled by ui_do_dates
        }
        if prev != self.settings {
            self.settings_changed(&prev);
            if prev.pet.color != self.settings.pet.color || prev.pet.character != self.settings.pet.character {
                if let Some(ui) = &self.ui {
                    set_window_icon(ui.hwnd, self.settings.pet.color.rgb(), self.settings.pet.character.shown(self.state.experience));
                }
            }
            self.ui_apply_chrome();
        }
    }

    /// Title bar colours follow the theme (Windows 11 DWM attributes; ignored elsewhere).
    pub fn ui_apply_chrome(&self) {
        let Some(ui) = &self.ui else { return };
        let pal = self.palette();
        unsafe {
            use windows_sys::Win32::Graphics::Dwm::*;
            let dark: i32 = pal.dark as i32;
            DwmSetWindowAttribute(ui.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, &dark as *const i32 as *const _, 4);
            let c = pal.window;
            let colorref: u32 = ((c & 0xff) << 16) | (c & 0xff00) | ((c >> 16) & 0xff);
            DwmSetWindowAttribute(ui.hwnd, DWMWA_CAPTION_COLOR as u32, &colorref as *const u32 as *const _, 4);
        }
    }
}


impl App {
    fn schedule_settings_save(&mut self) {
        self.mark_settings_dirty();
        self.schedule_save();
    }
}

fn local_count(app: &App) -> u32 {
    let today = pip_core::time::local(&chrono::Local, platform::now_unix()).date();
    if app.state.water.day == Some(today) { app.state.water.count } else { 0 }
}

/// What the "Daily goal" row says underneath itself: how today is going, which used to be a row
/// of its own called "Today" (§71).
fn water_today(app: &App) -> String {
    let mut t = format!("Glasses per day — {} so far today", local_count(app));
    if app.state.streak > 0 {
        t += &format!(", {}-day streak", app.state.streak);
    }
    t
}

pub fn set_window_icon(hwnd: HWND, body: u32, who: Character) {
    unsafe {
        for (which, size) in [(ICON_SMALL, 16 * 2), (ICON_BIG, 32 * 2)] {
            let icon = crate::tray::make_icon(body, who, false, size);
            let old = SendMessageW(hwnd, WM_SETICON, which as usize, icon as isize);
            if old != 0 {
                DestroyIcon(old as HICON);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Dates & Calendars pages
// ---------------------------------------------------------------------------------------------

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const LEADS: [(u32, &str); 4] = [(0, "On the day"), (1, "1 day before"), (3, "3 days before"), (7, "1 week before")];

fn max_day(month: u32) -> u32 {
    match month {
        2 => 29,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn ago(secs: i64) -> String {
    match secs {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

/// Calendar links are secrets: show only the host until the field is being edited.
fn masked_url(url: &str) -> String {
    let host = url.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or("");
    if url.is_empty() {
        String::new()
    } else {
        format!("{host}/ ••••••••  (hidden)")
    }
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}{:x}{:04x}", platform::now_unix(), platform::tick_ms() & 0xffff)
}

impl App {
    fn card(&self, c: &mut Ctx, h: f32, accent: bool) -> (f32, f32) {
        let s = c.s;
        let (x, y, w) = (c.x0 - 14.0 * s, c.y + 6.0 * s, c.x1 - c.x0 + 28.0 * s);
        fill_rrect(c.pm, x, y, w, h, 14.0 * s, color(c.pal.surface, 1.0));
        if accent {
            fill_rrect(c.pm, x + 5.0 * s, y + 16.0 * s, 3.5 * s, h - 32.0 * s, 2.0 * s, color(0xFFB020, 1.0));
        }
        (x, y)
    }

    /// Birthdays, anniversaries and holidays: the dates you keep yourself, as opposed to the
    /// calendars you subscribe to further down the page.
    fn dates_section(&self, c: &mut Ctx, focus: &Option<Field>) {
        let s = c.s;
        let f = fonts();
        let d = &self.settings.dates;
        let x1 = c.x1;
        let cy = c.row("Public holidays", Some("On today's card and in the morning summary"));
        let sel = if d.holidays == HolidayRegion::Za { 0 } else { 1 };
        c.segmented(cy, &["South Africa", "Off"], sel, vec![UiAct::Holidays(HolidayRegion::Za), UiAct::Holidays(HolidayRegion::None)]);
        let cy = c.row("Morning summary", Some("A quiet card with today's holidays, birthdays and events"));
        c.toggle(cy, d.morning_summary, UiAct::MorningSummary);
        let cy = c.row("Remind me at", Some("When date reminders and the summary appear"));
        c.stepper(cy, x1, 150.0, &d.notify_at.to_string(), UiAct::NotifyAt(-30), UiAct::NotifyAt(30));
        c.y += 12.0 * s;

        if d.items.is_empty() {
            c.y += 18.0 * s;
            let msg = "No dates yet. Add birthdays and anniversaries — give a birth year and Pip knows how old they're turning.";
            for line in text::wrap(&f.regular, 13.0 * s, msg, c.x1 - c.x0) {
                text::draw(c.pm, &f.regular, 13.0 * s, c.x0, c.y, c.pal.muted, 1.0, &line);
                c.y += 19.0 * s;
            }
            return;
        }
        let today = pip_core::time::local(&chrono::Local, platform::now_unix()).date();
        for item in &d.items {
            let card_h = 176.0 * s;
            let (_, y) = self.card(c, card_h, false);
            let id = item.id.clone();
            // Row 1: name + delete
            let del_w = ui::button_width("Delete", s);
            let tr = R::new(c.x0, y + 12.0 * s, c.x1 - c.x0 - del_w - 10.0 * s, 32.0 * s);
            ui::text_field(c.pm, tr, &item.title, *focus == Some(Field::DateTitle(id.clone())), &c.pal, s, "Name, e.g. Mom's birthday");
            c.hits.push((tr, UiAct::Focus(Field::DateTitle(id.clone()))));
            let dr = R::new(c.x1 - del_w, y + 14.0 * s, del_w, 28.0 * s);
            ui::button(c.pm, dr, "Delete", BtnStyle::Danger, &c.pal, s, c.hover == Some(dr));
            c.hits.push((dr, UiAct::DateDelete(id.clone())));
            // Row 2: kind
            let kinds = [DateKind::Birthday, DateKind::Anniversary, DateKind::Other, DateKind::Deadline];
            let labels: Vec<&str> = kinds.iter().map(|k| k.label()).collect();
            let sel = kinds.iter().position(|k| *k == item.kind).unwrap_or(0);
            for (r, k) in ui::segmented(c.pm, c.x0, y + 54.0 * s, &labels, sel, &c.pal, s).into_iter().zip(kinds) {
                c.hits.push((r, UiAct::DateKindSet(id.clone(), k)));
            }
            // Row 3: month / day / year + next occurrence
            let r3 = y + 94.0 * s;
            let (m, p) = ui::stepper(c.pm, c.x0, r3, 104.0 * s, MONTHS[(item.month.clamp(1, 12) - 1) as usize], &c.pal, s);
            c.hits.push((m, UiAct::DateMonth(id.clone(), -1)));
            c.hits.push((p, UiAct::DateMonth(id.clone(), 1)));
            let (m, p) = ui::stepper(c.pm, c.x0 + 112.0 * s, r3, 92.0 * s, &item.day.to_string(), &c.pal, s);
            c.hits.push((m, UiAct::DateDay(id.clone(), -1)));
            c.hits.push((p, UiAct::DateDay(id.clone(), 1)));
            let year = item.year.map(|y| y.to_string()).unwrap_or_else(|| "No year".into());
            let (m, p) = ui::stepper(c.pm, c.x0 + 212.0 * s, r3, 124.0 * s, &year, &c.pal, s);
            c.hits.push((m, UiAct::DateYear(id.clone(), -1)));
            c.hits.push((p, UiAct::DateYear(id.clone(), 1)));
            let next = match item.next_on_or_after(today) {
                Some(on) => {
                    let n = (on - today).num_days();
                    let when = match n {
                        0 => "Today!".to_string(),
                        1 => "Tomorrow".to_string(),
                        n => format!(
                            "{} {} {} · in {n} days",
                            &pip_core::time::weekday_name(chrono::Datelike::weekday(&on))[..3],
                            chrono::Datelike::day(&on),
                            MONTHS[chrono::Datelike::month0(&on) as usize]
                        ),
                    };
                    match item.count_on(on) {
                        Some(k) if item.kind == DateKind::Birthday => format!("{when} · turns {k}"),
                        Some(k) => format!("{when} · {k} years"),
                        None => when,
                    }
                }
                None => "Passed".into(),
            };
            let tx = c.x0 + 348.0 * s;
            let shown = text::ellipsize(&f.regular, 12.0 * s, &next, (c.x1 - tx).max(0.0));
            text::draw(c.pm, &f.regular, 12.0 * s, tx, r3 + 20.0 * s, c.pal.muted, 1.0, &shown);
            // Row 4: heads-up
            let labels: Vec<&str> = LEADS.iter().map(|(_, l)| *l).collect();
            let sel = LEADS.iter().position(|(d, _)| *d == item.lead_days).unwrap_or(0);
            for (r, (days, _)) in ui::segmented(c.pm, c.x0, y + 134.0 * s, &labels, sel, &c.pal, s).into_iter().zip(LEADS) {
                c.hits.push((r, UiAct::DateLead(id.clone(), days)));
            }
            c.y += card_h + 12.0 * s;
        }
    }

    fn page_calendars(&self, c: &mut Ctx, focus: &Option<Field>, kw_buf: &str) {
        let s = c.s;
        let f = fonts();
        section_label(c, "IMPORTANT DATES");
        self.dates_section(c, focus);
        // Connecting a calendar now happens on the calendar page, where you are already standing
        // when you want one (§71).
        section_label(c, "CALENDARS");
        self.draw_calendar_connectors(c);
        let d = &self.settings.dates;
        let now = platform::now_unix();
        let cy = c.row("Event alerts", Some("Important: has its own alert, important calendar, or a word below"));
        let sel = match d.event_alerts {
            EventAlerts::Important => 0,
            EventAlerts::All => 1,
            EventAlerts::Off => 2,
        };
        c.segmented(
            cy,
            &["Important", "All timed", "Off"],
            sel,
            vec![UiAct::Alerts(EventAlerts::Important), UiAct::Alerts(EventAlerts::All), UiAct::Alerts(EventAlerts::Off)],
        );
        let cy = c.row("Important words", Some("Comma-separated, e.g. interview, dentist"));
        let editing = *focus == Some(Field::Keywords);
        let value = if editing { kw_buf.to_string() } else { d.keywords.join(", ") };
        let r = R::new(c.x1 - 250.0 * s, cy - 16.0 * s, 250.0 * s, 32.0 * s);
        ui::text_field(c.pm, r, &value, editing, &c.pal, s, "No keywords");
        c.hits.push((r, UiAct::Focus(Field::Keywords)));
        // Only once there is something to sync. The row used to appear with "Add a calendar to
        // start" beside an empty space, which is a control that cannot be used telling you so.
        if !d.calendars.is_empty() {
            let last = d.calendars.iter().filter_map(|c| self.feed_meta.get(&c.id)).map(|m| m.synced_at).max().unwrap_or(0);
            let hint = if last > 0 { format!("Last synced {}. Checks hourly.", ago(now - last)) } else { "Not synced yet".to_string() };
            let cy = c.row("Sync", Some(&hint));
            let label = "Sync now";
            let bw = ui::button_width(label, s);
            c.button(c.x1 - bw, cy - 15.0 * s, label, BtnStyle::Secondary, UiAct::SyncNow);
        }
        c.y += 12.0 * s;

        for feed in &d.calendars {
            let card_h = 140.0 * s;
            let (_, y) = self.card(c, card_h, feed.important);
            let id = feed.id.clone();
            let nr = R::new(c.x0, y + 12.0 * s, c.x1 - c.x0 - 60.0 * s, 32.0 * s);
            ui::text_field(c.pm, nr, &feed.name, *focus == Some(Field::CalName(id.clone())), &c.pal, s, "Calendar name");
            c.hits.push((nr, UiAct::Focus(Field::CalName(id.clone()))));
            let tg = ui::toggle(c.pm, c.x1 - ui::TOGGLE_W * s, y + 17.0 * s, feed.enabled, &c.pal, s);
            c.hits.push((tg, UiAct::CalOn(id.clone())));
            // A Google-connected calendar has no link to show or edit — just who it belongs to.
            if let Some(address) = pip_core::gcal::feed_address(&feed.url) {
                text::draw(c.pm, &f.regular, 12.5 * s, c.x0 + 2.0 * s, y + 72.0 * s, c.pal.muted, 1.0, &format!("Connected with Google as {address}"));
            } else {
                let ur = R::new(c.x0, y + 52.0 * s, c.x1 - c.x0, 32.0 * s);
                let editing_url = *focus == Some(Field::CalUrl(id.clone()));
                let shown = if editing_url { feed.url.clone() } else { masked_url(&feed.url) };
                ui::text_field(c.pm, ur, &shown, editing_url, &c.pal, s, "Paste the calendar's private iCal / webcal link");
                c.hits.push((ur, UiAct::Focus(Field::CalUrl(id.clone()))));
            }
            // Row 3: status + actions
            let r3 = y + 94.0 * s;
            let meta = self.feed_meta.get(&feed.id);
            let (status, is_err) = if feed.url.trim().is_empty() {
                ("Paste a link above to start syncing".to_string(), false)
            } else if !feed.enabled {
                ("Paused".to_string(), false)
            } else if let Some(e) = meta.and_then(|m| m.error.clone()) {
                (e, true)
            } else if let Some(m) = meta.filter(|m| m.synced_at > 0) {
                (format!("Synced {} · {} events in the next two weeks", ago(now - m.synced_at), m.events), false)
            } else {
                ("Waiting to sync…".to_string(), false)
            };
            let del_w = ui::button_width("Delete", s);
            let imp = if feed.important { "Important  ✓" } else { "Mark important" };
            let imp_w = ui::button_width(imp, s);
            let avail = c.x1 - c.x0 - del_w - imp_w - 24.0 * s;
            let status = text::ellipsize(&f.regular, 12.0 * s, &status, avail.max(0.0));
            text::draw(c.pm, &f.regular, 12.0 * s, c.x0, r3 + 19.0 * s, if is_err { 0xD6455D } else { c.pal.muted }, 1.0, &status);
            let dr = R::new(c.x1 - del_w, r3, del_w, 28.0 * s);
            ui::button(c.pm, dr, "Delete", BtnStyle::Danger, &c.pal, s, c.hover == Some(dr));
            c.hits.push((dr, UiAct::CalDelete(id.clone())));
            let ir = R::new(dr.x - imp_w - 6.0 * s, r3, imp_w, 28.0 * s);
            ui::button(c.pm, ir, imp, if feed.important { BtnStyle::Primary } else { BtnStyle::Secondary }, &c.pal, s, c.hover == Some(ir));
            c.hits.push((ir, UiAct::CalImportant(id)));
            c.y += card_h + 12.0 * s;
        }
        c.y += 6.0 * s;
        let privacy = "Pip only ever reads your calendars — it never changes them. Connect or disconnect in Settings › Connections.";
        for line in text::wrap(&f.regular, 12.0 * s, privacy, c.x1 - c.x0) {
            c.y += 18.0 * s;
            text::draw(c.pm, &f.regular, 12.0 * s, c.x0, c.y, c.pal.muted, 1.0, &line);
        }
    }

    /// Handles the Dates/Calendars actions. Returns false for actions it doesn't own.
    fn ui_do_dates(&mut self, act: &UiAct) -> bool {
        let now = platform::now_unix();
        let year_now = chrono::Datelike::year(&pip_core::time::local(&chrono::Local, now).date());
        let d = &mut self.settings.dates;
        match act {
            UiAct::Holidays(r) => d.holidays = *r,
            UiAct::MorningSummary => d.morning_summary = !d.morning_summary,
            UiAct::NotifyAt(m) => d.notify_at = d.notify_at.add_minutes(*m),
            UiAct::AddDate => {
                let today = pip_core::time::local(&chrono::Local, now).date();
                let id = new_id("d");
                d.items.push(ImportantDate {
                    id: id.clone(),
                    month: chrono::Datelike::month(&today),
                    day: chrono::Datelike::day(&today),
                    lead_days: 1,
                    updated_at: now,
                    ..Default::default()
                });
                if let Some(ui) = &mut self.ui {
                    ui.focus = Some(Field::DateTitle(id));
                    ui.scroll = f32::MAX;
                }
            }
            UiAct::DateKindSet(id, k) => {
                if let Some(x) = d.items.iter_mut().find(|x| &x.id == id) {
                    x.kind = *k;
                    if *k == DateKind::Deadline && x.year.is_none() {
                        x.year = Some(year_now);
                    }
                    x.updated_at = now;
                }
            }
            UiAct::DateMonth(id, delta) => {
                if let Some(x) = d.items.iter_mut().find(|x| &x.id == id) {
                    x.month = ((x.month as i32 - 1 + delta).rem_euclid(12) + 1) as u32;
                    x.day = x.day.min(max_day(x.month));
                    x.updated_at = now;
                }
            }
            UiAct::DateDay(id, delta) => {
                if let Some(x) = d.items.iter_mut().find(|x| &x.id == id) {
                    let m = max_day(x.month) as i32;
                    x.day = ((x.day as i32 - 1 + delta).rem_euclid(m) + 1) as u32;
                    x.updated_at = now;
                }
            }
            UiAct::DateYear(id, delta) => {
                if let Some(x) = d.items.iter_mut().find(|x| &x.id == id) {
                    x.year = match x.year {
                        None => Some(if x.kind == DateKind::Deadline { year_now } else { year_now - 30 }),
                        Some(y) if y + delta < 1900 && x.kind != DateKind::Deadline => None,
                        Some(y) => Some((y + delta).clamp(1900, year_now + 10)),
                    };
                    x.updated_at = now;
                }
            }
            UiAct::DateLead(id, l) => {
                if let Some(x) = d.items.iter_mut().find(|x| &x.id == id) {
                    x.lead_days = *l;
                    x.updated_at = now;
                }
            }
            UiAct::DateDelete(id) => d.items.retain(|x| &x.id != id),
            UiAct::Alerts(a) => d.event_alerts = *a,
            UiAct::CalOn(id) => {
                if let Some(x) = d.calendars.iter_mut().find(|x| &x.id == id) {
                    x.enabled = !x.enabled;
                    x.updated_at = now;
                }
            }
            UiAct::CalImportant(id) => {
                if let Some(x) = d.calendars.iter_mut().find(|x| &x.id == id) {
                    x.important = !x.important;
                    x.updated_at = now;
                }
            }
            UiAct::CalDelete(id) => d.calendars.retain(|x| &x.id != id),
            UiAct::SyncNow => self.start_sync(true),
            _ => return false,
        }
        true
    }
}

// ---------------------------------------------------------------------------------------------
// Email page
// ---------------------------------------------------------------------------------------------

impl App {
    /// The Gmail connector card: one button to connect, then the connected account(s).
    /// Shown on both Connections and Email so it's never more than one click away.
    ///
    /// Design rule (user's): connecting must feel like "Continue with Google → choose account →
    /// Allow → done". No guides, keys or setup in front of the user; Google's own screens are the
    /// only steps, and Pip only adds a one-line heads-up for Google's unavoidable warning.
    fn draw_gmail_section(&self, c: &mut Ctx) {
        let s = c.s;
        let f = fonts();
        let inner = c.x1 - c.x0;
        let conn = connectors::get(ConnectorId::Gmail);
        let accounts = &self.settings.email.accounts;
        let available = self.google_keys().is_some();
        let waiting = match &self.signin {
            SignIn::Waiting { connector, reconnecting } if *connector == ConnectorId::Gmail => Some(reconnecting.clone()),
            _ => None,
        };
        let problem = match &self.signin {
            SignIn::Problem(ConnectorId::Gmail, p) => Some(p),
            _ => None,
        };

        // Measure.
        let note = if !available {
            Some((c.pal.muted, "Google sign-in isn\u{2019}t included in this copy of Pip.".to_string()))
        } else if waiting.is_some() {
            Some((c.pal.muted, SIGNIN_HINT.to_string()))
        } else {
            problem.map(|p| (if p.is_error() { ERR_RED } else { c.pal.muted }, p.explain("Google")))
        };
        let access_lines = if accounts.is_empty() { text::wrap(&f.regular, 12.0 * s, conn.access, inner) } else { Vec::new() };
        let note_lines = note.as_ref().map(|(_, t)| text::wrap(&f.regular, 12.0 * s, t, inner - 16.0 * s)).unwrap_or_default();
        let wait_h = if waiting.is_some() { 44.0 * s } else { 0.0 };
        let mut h = 74.0 * s + access_lines.len() as f32 * 18.0 * s + wait_h;
        if !note_lines.is_empty() {
            h += note_lines.len() as f32 * 18.0 * s + 8.0 * s;
        }
        h += accounts.len() as f32 * 66.0 * s + if accounts.is_empty() { 4.0 * s } else { 8.0 * s };

        let (_, y) = self.card(c, h, false);
        let connected = accounts.iter().any(|a| !self.mail_status.get(&a.id).is_some_and(|st| st.signed_out));
        self.connector_header(c, y, conn, if connected { Some((OK_GREEN, "Connected")) } else { None });

        // The one action, top right.
        let main = if waiting.is_some() {
            Some(("Cancel", BtnStyle::Ghost, UiAct::GmailCancel))
        } else if !available {
            None
        } else if accounts.is_empty() {
            Some(("Continue with Google", BtnStyle::Primary, UiAct::GmailSignIn))
        } else if accounts.len() < 5 {
            Some(("Add account", BtnStyle::Secondary, UiAct::AddMailAccount))
        } else {
            None
        };
        if let Some((label, style, act)) = main {
            let bw = ui::button_width(label, s);
            c.button(c.x1 - bw, y + 21.0 * s, label, style, act);
        }

        let mut ty = y + 70.0 * s;
        for l in &access_lines {
            ty += 18.0 * s;
            text::draw(c.pm, &f.regular, 12.0 * s, c.x0, ty - 4.0 * s, c.pal.muted, 1.0, l);
        }

        // Waiting: one line, plus a way back to the browser if its window got buried or closed.
        if let Some(reconnecting) = &waiting {
            let label = match reconnecting {
                Some(a) => format!("Continue in your browser to reconnect {a}\u{2026}"),
                None => "Continue in your browser\u{2026}".to_string(),
            };
            let bw = ui::button_width("Open it again", s);
            let label = text::ellipsize(&f.semibold, 13.0 * s, &label, inner - bw - 16.0 * s);
            text::draw(c.pm, &f.semibold, 13.0 * s, c.x0, ty + 26.0 * s, c.pal.accent, 1.0, &label);
            c.button(c.x1 - bw, ty + 6.0 * s, "Open it again", BtnStyle::Secondary, UiAct::GmailReopen);
            ty += wait_h;
        }

        // A short note: the heads-up while waiting, or why the last attempt didn't work.
        if let Some((rgb, _)) = &note {
            ty += 4.0 * s;
            for l in &note_lines {
                ty += 18.0 * s;
                text::draw(c.pm, &f.regular, 12.0 * s, c.x0, ty - 4.0 * s, *rgb, 1.0, l);
            }
            ty += 4.0 * s;
        }

        // Connected accounts.
        let now = platform::now_unix();
        for a in accounts {
            let st = self.mail_status.get(&a.id).cloned().unwrap_or_default();
            ty += 4.0 * s;
            fill_rrect(c.pm, c.x0, ty, inner, 1.0, 0.0, color(c.pal.border, 0.8));
            let reconnecting_this = waiting.as_ref().is_some_and(|w| w.as_deref() == Some(a.address.as_str()));
            let pending = self.state.mail_pending.iter().filter(|m| m.account_id == a.id).count();
            let last = self.state.mail.get(&a.id).map(|m| m.last_check).unwrap_or(0).max(st.checked_at);
            let (dot, status) = if reconnecting_this {
                (WARN_AMBER, "Reconnecting\u{2026}".to_string())
            } else if st.signed_out {
                (WARN_AMBER, "Signed out \u{2014} click Reconnect".to_string())
            } else if let Some(e) = &st.error {
                (ERR_RED, e.clone())
            } else if last > 0 {
                let mut t = format!("Connected \u{00b7} checked {}", ago(now - last));
                if pending > 0 {
                    t += &format!(" \u{00b7} {pending} important waiting");
                }
                (OK_GREEN, t)
            } else {
                (OK_GREEN, "Connected".to_string())
            };
            let confirming = self.confirm_disconnect.as_deref() == Some(a.id.as_str());
            let dlabel = if confirming { "Yes, disconnect" } else { "Disconnect" };
            let dw = ui::button_width(dlabel, s);
            let (label, style, act) = if st.signed_out {
                ("Reconnect", BtnStyle::Primary, UiAct::GmailReconnect(a.address.clone()))
            } else {
                ("Check now", BtnStyle::Secondary, UiAct::MailCheckNow)
            };
            let cw = ui::button_width(label, s);
            let row_y = ty;
            crate::render::fill_circle(c.pm, c.x0 + 5.0 * s, row_y + 23.0 * s, 4.5 * s, color(dot, 1.0));
            let avail = inner - dw - cw - 36.0 * s;
            let addr = text::ellipsize(&f.semibold, 13.5 * s, &a.address, avail.max(0.0));
            text::draw(c.pm, &f.semibold, 13.5 * s, c.x0 + 18.0 * s, row_y + 28.0 * s, c.pal.text, 1.0, &addr);
            let shown = text::ellipsize(&f.regular, 12.0 * s, &status, avail.max(0.0));
            text::draw(c.pm, &f.regular, 12.0 * s, c.x0 + 18.0 * s, row_y + 47.0 * s, if dot == ERR_RED { ERR_RED } else { c.pal.muted }, 1.0, &shown);
            c.button(c.x1 - dw, row_y + 17.0 * s, dlabel, BtnStyle::Danger, UiAct::MailDisconnect(a.id.clone()));
            if !reconnecting_this {
                c.button(c.x1 - dw - cw - 8.0 * s, row_y + 17.0 * s, label, style, act);
            }
            ty += 62.0 * s;
        }
        c.y += h + 12.0 * s;
    }

    /// Badge, name, purpose line and an optional status chip — the top of every connector card.
    fn connector_header(&self, c: &mut Ctx, y: f32, conn: &Connector, chip: Option<(u32, &str)>) {
        let s = c.s;
        let f = fonts();
        badge(c, c.x0 + 17.0 * s, y + 36.0 * s, conn.badge, conn.badge_rgb);
        let nx = c.x0 + 46.0 * s;
        let nw = text::draw(c.pm, &f.semibold, 14.5 * s, nx, y + 32.0 * s, c.pal.text, 1.0, conn.name);
        if let Some((rgb, label)) = chip {
            let cx = nx + nw + 12.0 * s;
            crate::render::fill_circle(c.pm, cx + 4.0 * s, y + 27.5 * s, 4.0 * s, color(rgb, 1.0));
            text::draw(c.pm, &f.semibold, 12.0 * s, cx + 13.0 * s, y + 32.0 * s, rgb, 1.0, label);
        }
        text::draw(c.pm, &f.regular, 12.0 * s, nx, y + 51.0 * s, c.pal.muted, 1.0, conn.purpose);
    }

    /// Calendar connectors: Google signs in like Gmail does; the others use the guided link flow
    /// (Outlook and iCloud offer no sign-in for calendars, so a private link is the best there is).
    fn draw_calendar_connectors(&self, c: &mut Ctx) {
        let s = c.s;
        let f = fonts();
        let feeds = &self.settings.dates.calendars;
        let rows: Vec<&Connector> = connectors::ALL.iter().filter(|x| x.id == ConnectorId::GoogleCalendar).collect();
        let google_feeds: Vec<_> = feeds.iter().filter(|fd| pip_core::gcal::feed_address(&fd.url).is_some()).collect();
        let waiting_google = matches!(&self.signin, SignIn::Waiting { connector: ConnectorId::GoogleCalendar, .. });
        let google_problem = match &self.signin {
            SignIn::Problem(ConnectorId::GoogleCalendar, p) => Some(p),
            _ => None,
        };
        let extra_google = google_feeds.len() as f32 * 44.0 * s + if waiting_google || google_problem.is_some() { 24.0 * s } else { 0.0 };
        let row_h = 70.0 * s;
        let h = rows.len() as f32 * row_h + extra_google + 8.0 * s;
        let (_, y) = self.card(c, h, false);
        let mut ry = y + 4.0 * s;
        for (i, conn) in rows.iter().enumerate() {
            if i > 0 {
                fill_rrect(c.pm, c.x0, ry, c.x1 - c.x0, 1.0, 0.0, color(c.pal.border, 0.8));
            }
            {
                // Google Calendar: one click, like Gmail.
                {
                    let signed_out = google_feeds.iter().any(|fd| self.feed_meta.get(&fd.id).is_some_and(|m| m.signed_out));
                    let chip = match (google_feeds.is_empty(), signed_out) {
                        (true, _) => None,
                        (_, true) => Some((WARN_AMBER, "Needs attention")),
                        _ => Some((OK_GREEN, "Connected")),
                    };
                    self.connector_header(c, ry - 4.0 * s, conn, chip);
                    let main = if waiting_google {
                        Some(("Cancel", BtnStyle::Ghost, UiAct::GmailCancel))
                    } else if self.google_keys().is_none() {
                        None
                    } else if google_feeds.is_empty() {
                        Some(("Continue with Google", BtnStyle::Primary, UiAct::ConnectGoogleCalendar(None)))
                    } else {
                        Some(("Add account", BtnStyle::Secondary, UiAct::ConnectGoogleCalendar(Some(String::new()))))
                    };
                    if let Some((label, style, act)) = main {
                        let bw = ui::button_width(label, s);
                        c.button(c.x1 - bw, ry + 18.0 * s, label, style, act);
                    }
                    ry += row_h;
                    if waiting_google || google_problem.is_some() {
                        let (rgb, msg) = match google_problem {
                            Some(p) => (if p.is_error() { ERR_RED } else { c.pal.muted }, p.explain("Google")),
                            None => (c.pal.accent, "Continue in your browser\u{2026}".to_string()),
                        };
                        let msg = text::ellipsize(&f.regular, 12.0 * s, &msg, c.x1 - c.x0);
                        text::draw(c.pm, &f.regular, 12.0 * s, c.x0, ry - 8.0 * s, rgb, 1.0, &msg);
                        ry += 24.0 * s;
                    }
                    // One line per connected Google account.
                    for fd in &google_feeds {
                        let meta = self.feed_meta.get(&fd.id);
                        let out = meta.is_some_and(|m| m.signed_out);
                        let address = pip_core::gcal::feed_address(&fd.url).unwrap_or(&fd.name);
                        crate::render::fill_circle(c.pm, c.x0 + 5.0 * s, ry + 14.0 * s, 4.5 * s, color(if out { WARN_AMBER } else { OK_GREEN }, 1.0));
                        let status = if out {
                            "Signed out \u{2014} click Reconnect".to_string()
                        } else {
                            match meta.filter(|m| m.synced_at > 0) {
                                Some(m) => format!("{address} \u{00b7} {} events \u{00b7} synced {}", m.events, ago(platform::now_unix() - m.synced_at)),
                                None => format!("{address} \u{00b7} syncing\u{2026}"),
                            }
                        };
                        let dlabel = if self.confirm_disconnect.as_deref() == Some(fd.id.as_str()) { "Yes, disconnect" } else { "Disconnect" };
                        let dw = ui::button_width(dlabel, s);
                        let rw = ui::button_width("Reconnect", s);
                        let avail = c.x1 - c.x0 - dw - 24.0 * s - if out { rw + 8.0 * s } else { 0.0 };
                        let shown = text::ellipsize(&f.regular, 12.5 * s, &status, avail.max(0.0));
                        text::draw(c.pm, &f.regular, 12.5 * s, c.x0 + 18.0 * s, ry + 19.0 * s, c.pal.muted, 1.0, &shown);
                        c.button(c.x1 - dw, ry + 2.0 * s, dlabel, BtnStyle::Danger, UiAct::CalDelete(fd.id.clone()));
                        if out {
                            c.button(c.x1 - dw - rw - 8.0 * s, ry + 2.0 * s, "Reconnect", BtnStyle::Primary, UiAct::ConnectGoogleCalendar(Some(address.to_string())));
                        }
                        ry += 44.0 * s;
                    }
                }
            }
        }
        c.y += h + 12.0 * s;
    }

    /// Phone sync: connect, then let other devices in with a single Approve.
    fn draw_sync_connector(&self, c: &mut Ctx) {
        let s = c.s;
        let f = fonts();
        let inner = c.x1 - c.x0;
        let conn = connectors::get(ConnectorId::PhoneSync);
        let on = self.sync_connected();
        let waiting = matches!(&self.signin, SignIn::Waiting { connector: ConnectorId::PhoneSync, .. });
        let problem = match &self.signin {
            SignIn::Problem(ConnectorId::PhoneSync, p) => Some(p),
            _ => None,
        };
        let asking = if on { self.sync_requests.clone() } else { Vec::new() };
        let status = if self.sync_status.signed_out {
            Some((WARN_AMBER, "Signed out — connect again".to_string()))
        } else if let Some(e) = &self.sync_status.error {
            Some((ERR_RED, e.clone()))
        } else if self.sync_status.last_at > 0 {
            let from = if self.sync_status.last_device.is_empty() { String::new() } else { format!(" · last change from {}", self.sync_status.last_device) };
            Some((c.pal.muted, format!("Synced {}{from}", ago(platform::now_unix() - self.sync_status.last_at))))
        } else if self.sync_refused {
            Some((WARN_AMBER, "Your other device said not now".to_string()))
        } else if self.sync_waiting {
            Some((c.pal.muted, "Waiting to be let in…".to_string()))
        } else if on {
            Some((c.pal.muted, "Syncing shortly…".to_string()))
        } else {
            None
        };

        let access_lines = if on { Vec::new() } else { text::wrap(&f.regular, 12.0 * s, conn.access, inner) };
        let note = if waiting {
            Some((c.pal.muted, SIGNIN_HINT.to_string()))
        } else {
            problem.map(|p| (if p.is_error() { ERR_RED } else { c.pal.muted }, p.explain("Google")))
        };
        let note_lines = note.as_ref().map(|(_, t)| text::wrap(&f.regular, 12.0 * s, t, inner - 16.0 * s)).unwrap_or_default();
        let mut h = 74.0 * s + access_lines.len() as f32 * 18.0 * s + note_lines.len() as f32 * 18.0 * s;
        if status.is_some() {
            h += 26.0 * s;
        }
        if on {
            h += asking.len() as f32 * 56.0 * s;
            if self.sync_waiting || self.sync_refused {
                h += 56.0 * s;
            }
            h += 44.0 * s; // the Disconnect row
        }
        h += 8.0 * s;

        let (_, y) = self.card(c, h, false);
        let badge = if self.sync_ready() { Some((OK_GREEN, "On")) } else if on { Some((c.pal.muted, "Setting up")) } else { None };
        self.connector_header(c, y, conn, badge);
        let main = if waiting {
            Some(("Cancel", BtnStyle::Ghost, UiAct::GmailCancel))
        } else if self.google_keys().is_none() {
            None
        } else if on {
            Some(("Sync now", BtnStyle::Secondary, UiAct::PhoneSyncNow))
        } else {
            Some(("Continue with Google", BtnStyle::Primary, UiAct::SyncConnect))
        };
        if let Some((label, style, act)) = main {
            let bw = ui::button_width(label, s);
            c.button(c.x1 - bw, y + 21.0 * s, label, style, act);
        }
        let mut ty = y + 70.0 * s;
        for l in &access_lines {
            ty += 18.0 * s;
            text::draw(c.pm, &f.regular, 12.0 * s, c.x0, ty - 4.0 * s, c.pal.muted, 1.0, l);
        }
        for l in &note_lines {
            ty += 18.0 * s;
            let rgb = note.as_ref().map(|(r, _)| *r).unwrap_or(c.pal.muted);
            text::draw(c.pm, &f.regular, 12.0 * s, c.x0, ty - 4.0 * s, rgb, 1.0, l);
        }
        if let Some((rgb, text_)) = &status {
            ty += 22.0 * s;
            let shown = text::ellipsize(&f.regular, 12.0 * s, text_, inner - 120.0 * s);
            text::draw(c.pm, &f.regular, 12.0 * s, c.x0, ty - 4.0 * s, *rgb, 1.0, &shown);
        }
        if on {
            // Anyone asking to be let in, one tap each.
            for r in &asking {
                ty += 12.0 * s;
                fill_rrect(c.pm, c.x0, ty, inner, 44.0 * s, 10.0 * s, color(c.pal.surface2, 1.0));
                let aw = ui::button_width("Approve", s);
                let nw = ui::button_width("Not now", s);
                let title = format!("{} wants to sync", r.device);
                let shown = text::ellipsize(&f.semibold, 13.0 * s, &title, (inner - aw - nw - 48.0 * s).max(0.0));
                text::draw(c.pm, &f.semibold, 13.0 * s, c.x0 + 14.0 * s, ty + 19.0 * s, c.pal.text, 1.0, &shown);
                text::draw(c.pm, &f.regular, 11.5 * s, c.x0 + 14.0 * s, ty + 35.0 * s, c.pal.muted, 1.0, "Approve only if you just signed in there.");
                c.button(c.x1 - aw - 8.0 * s, ty + 8.0 * s, "Approve", BtnStyle::Primary, UiAct::SyncApprove(r.id.clone()));
                c.button(c.x1 - aw - nw - 16.0 * s, ty + 8.0 * s, "Not now", BtnStyle::Ghost, UiAct::SyncDeny(r.id.clone()));
                ty += 44.0 * s;
            }
            if self.sync_refused {
                ty += 12.0 * s;
                fill_rrect(c.pm, c.x0, ty, inner, 44.0 * s, 10.0 * s, color(c.pal.surface2, 1.0));
                text::draw(c.pm, &f.semibold, 13.0 * s, c.x0 + 14.0 * s, ty + 19.0 * s, c.pal.text, 1.0, "Not approved");
                text::draw(c.pm, &f.regular, 11.5 * s, c.x0 + 14.0 * s, ty + 35.0 * s, c.pal.muted, 1.0, "Your other device said not now. You can ask it again.");
                let aw = ui::button_width("Ask again", s);
                c.button(c.x1 - aw - 8.0 * s, ty + 8.0 * s, "Ask again", BtnStyle::Primary, UiAct::SyncAskAgain);
                ty += 44.0 * s;
            } else if self.sync_waiting {
                ty += 12.0 * s;
                fill_rrect(c.pm, c.x0, ty, inner, 44.0 * s, 10.0 * s, color(c.pal.surface2, 1.0));
                text::draw(c.pm, &f.semibold, 13.0 * s, c.x0 + 14.0 * s, ty + 19.0 * s, c.pal.text, 1.0, "Waiting for your other device");
                text::draw(c.pm, &f.regular, 11.5 * s, c.x0 + 14.0 * s, ty + 35.0 * s, c.pal.muted, 1.0, "Open Pip there and tap Approve. Nothing to type.");
                ty += 44.0 * s;
            }
            ty += 14.0 * s;
            let dlabel = if self.confirm_disconnect.as_deref() == Some("sync") { "Yes, disconnect" } else { "Disconnect" };
            let dw = ui::button_width(dlabel, s);
            c.button(c.x1 - dw, ty, dlabel, BtnStyle::Danger, UiAct::SyncDisconnect);
        }
        c.y += h + 12.0 * s;
    }

    fn page_email(&self, c: &mut Ctx, focus: &Option<Field>, list_buf: &str) {
        let s = c.s;
        let f = fonts();
        let inner = c.x1 - c.x0;
        c.y += 6.0 * s;
        self.draw_gmail_section(c);

        // Rules
        let e = &self.settings.email;
        let list_field = |c: &mut Ctx, label: &str, hint: &str, field: Field, list: &Vec<String>, placeholder: &str| {
            let cy = c.row(label, Some(hint));
            let editing = *focus == Some(field.clone());
            let value = if editing { list_buf.to_string() } else { list.join(", ") };
            let r = R::new(c.x1 - 270.0 * s, cy - 16.0 * s, 270.0 * s, 32.0 * s);
            ui::text_field(c.pm, r, &value, editing, &c.pal, s, placeholder);
            c.hits.push((r, UiAct::Focus(field)));
        };
        list_field(c, "Important senders", "Addresses, @domains or names, comma-separated", Field::MailSenders, &e.senders, "e.g. mom@gmail.com, @work.co.za");
        list_field(c, "Subject words", "Mail whose subject contains any of these", Field::MailKeywords, &e.keywords, "e.g. urgent, invoice");
        c.y += 8.0 * s;
        // Three rows became two and a sentence (§71). "Check every" was never a decision anybody
        // wanted; Gmail's own Important marker warned in its own hint that it "can be a lot"; and
        // a star is the one mark you put there yourself, so it counts from the first day rather
        // than waiting behind a switch nobody found. Worded carefully, because Pip looks at mail
        // as it arrives and only ever sees a star that is already on the message — starring
        // something later does not bring it back, which is what Important on the card is for.
        let how = "Pip also nudges for mail you had already starred in Gmail, and checks every few minutes while you\u{2019}re at the PC \u{2014} never while you\u{2019}re away or locked. Anything else, use Important on the pop-up when it arrives.";
        for line in text::wrap(&f.regular, 12.5 * s, how, inner) {
            c.y += 19.0 * s;
            text::draw(c.pm, &f.regular, 12.5 * s, c.x0, c.y, c.pal.muted, 1.0, &line);
        }
        c.y += 8.0 * s;
        let privacy = "Pip asks Google only for each new email\u{2019}s sender, subject and labels \u{2014} it can\u{2019}t read, send or delete your mail. Sign-ins are stored encrypted on this PC; nothing goes anywhere else.";
        for line in text::wrap(&f.regular, 12.0 * s, privacy, inner) {
            c.y += 18.0 * s;
            text::draw(c.pm, &f.regular, 12.0 * s, c.x0, c.y, c.pal.muted, 1.0, &line);
        }
    }

    fn ui_do_email(&mut self, act: &UiAct) -> bool {
        let prev = self.settings.clone();
        if !matches!(act, UiAct::MailDisconnect(_) | UiAct::CalDelete(_) | UiAct::SyncDisconnect) {
            self.confirm_disconnect = None;
        }
        match act {
            UiAct::GmailSignIn => self.start_gmail_signin(None, false),
            UiAct::AddMailAccount => self.start_gmail_signin(None, true),
            UiAct::GmailReconnect(address) => self.start_gmail_signin(Some(address.clone()), false),
            UiAct::GmailReopen => self.reopen_signin(),
            UiAct::GmailCancel => self.cancel_gmail_signin(),
            UiAct::MailCheckNow => self.check_mail(true),
            UiAct::MailDisconnect(id) => {
                // Two clicks: the first turns the button into "Yes, disconnect".
                if self.confirm_disconnect.as_deref() == Some(id.as_str()) {
                    self.disconnect_mail(id);
                } else {
                    self.confirm_disconnect = Some(id.clone());
                }
            }
            UiAct::SyncConnect => self.start_signin(ConnectorId::PhoneSync, None, false),
            UiAct::SyncApprove(id) => self.approve_join(id),
            UiAct::SyncDeny(id) => self.deny_join(id),
            UiAct::SyncAskAgain => self.ask_to_join_again(),
            UiAct::PhoneSyncNow => self.sync_now(true),
            UiAct::SyncDisconnect => {
                // Two clicks, like the other disconnects.
                if self.confirm_disconnect.as_deref() == Some("sync") {
                    self.disconnect_sync();
                } else {
                    self.confirm_disconnect = Some("sync".into());
                }
            }
            UiAct::ConnectGoogleCalendar(which) => {
                let (hint, choose) = match which.as_deref() {
                    None => (None, false),
                    Some("") => (None, true),
                    Some(a) => (Some(a.to_string()), false),
                };
                self.start_signin(ConnectorId::GoogleCalendar, hint, choose);
            }
            UiAct::CalDelete(id) if pip_core::gcal::feed_address(self.settings.dates.calendars.iter().find(|c| &c.id == id).map(|c| c.url.as_str()).unwrap_or("")).is_some() => {
                // Disconnecting a Google calendar: confirm once, then remove it and hand back access.
                if self.confirm_disconnect.as_deref() == Some(id.as_str()) {
                    let prev = self.settings.clone();
                    self.settings.dates.calendars.retain(|c| &c.id != id);
                    self.settings_changed(&prev);
                    self.confirm_disconnect = None;
                } else {
                    self.confirm_disconnect = Some(id.clone());
                }
            }
            UiAct::Page(_) => {
                // Leaving the page puts an old "didn't work" message away.
                self.dismiss_signin_problem();
                return false;
            }
            _ => return false,
        }
        if prev != self.settings {
            self.settings_changed(&prev);
            self.schedule_mail();
        }
        self.ui_invalidate();
        true
    }
}

const OK_GREEN: u32 = 0x3BB08F;
const WARN_AMBER: u32 = 0xE0A030;
const ERR_RED: u32 = 0xD6455D;

/// The only thing worth saying while Google's screens are open: its warning for apps that haven't
/// been through Google's paid review can't be avoided, so say what to click.
const SIGNIN_HINT: &str = "If Google says it hasn\u{2019}t verified Pip, choose Continue.";

/// A round letter badge standing in for the service's logo.
fn badge(c: &mut Ctx, cx: f32, cy: f32, label: &str, rgb: u32) {
    let s = c.s;
    let f = fonts();
    crate::render::fill_circle(c.pm, cx, cy, 17.0 * s, color(rgb, 1.0));
    let fs = if label.chars().count() > 1 { 12.5 * s } else { 15.5 * s };
    text::draw_centered(c.pm, &f.semibold, fs, cx, cy + fs * 0.36, 0xFFFFFF, 1.0, label);
}

fn section_label(c: &mut Ctx, label: &str) {
    let s = c.s;
    c.y += 20.0 * s;
    text::draw(c.pm, &fonts().semibold, 11.0 * s, c.x0, c.y, c.pal.muted, 1.0, label);
}


#[cfg(test)]
mod character_row_fits {
    use super::*;

    /// The content column, in the same units everything else here is drawn in.
    fn content(s: f32) -> (f32, f32) {
        let x0 = (SIDEBAR_W + 36.0) * s;
        let x1 = LOGICAL_W as f32 * s - 36.0 * s;
        (x0, x1)
    }

    /// Every width in this window is a constant times `s` — the window itself is created at
    /// `LOGICAL_W * s` — so a row that fits at 100% fits at every scaling. The three scales are
    /// checked anyway, because that argument is only true while it stays true.
    #[test]
    fn the_characters_fit_beside_their_own_label() {
        let f = fonts();
        for s in [1.0f32, 1.25, 1.5] {
            let (x0, x1) = content(s);
            let hint = text::width(&f.regular, 12.0 * s, "Who your companion is — the rest stays the same");
            for (i, chunk) in Character::ALL.chunks(4).enumerate() {
                let labels: Vec<&str> = chunk.iter().map(|x| x.label()).collect();
                let w = ui::segmented_width(&labels, s);
                // The first line shares its row with the label and hint; the rest have the row to
                // themselves and only have to stay inside the column.
                let left = if i == 0 { x0 + hint + 12.0 * s } else { x0 };
                assert!(
                    x1 - w >= left,
                    "at {s}x, character line {i} ({labels:?}) is {:.0}px too wide and would run back over the text",
                    left - (x1 - w)
                );
            }
        }
    }

    /// §71 put "Starting at" and "Until" onto one row, because they were always two halves of one
    /// decision rather than two decisions. Two steppers plus a gap is 308 units of the right-hand
    /// side, where every other row spends 150, so the label and its hint have to be checked
    /// against what is left — and checked at every scaling, since a hint that fits at 100% is the
    /// kind of thing that stops fitting at 150% and is never looked at again.
    #[test]
    fn two_times_on_one_row_leave_their_hint_room() {
        let f = fonts();
        // Both the water window and do-not-disturb; the longer hint is the one that decides it.
        for (label, hint) in [
            ("Between", "No water reminders outside these hours"),
            ("Do not disturb", "Reminders wait, except important ones. Your companion sleeps either way"),
        ] {
            for s in [1.0f32, 1.25, 1.5] {
                let (x0, x1) = content(s);
                let text_w = text::width(&f.semibold, 13.5 * s, label).max(text::width(&f.regular, 12.0 * s, hint));
                // A stepper at `x_right` occupies `w * s` back from it; the pair sits at
                // `x1 - 158 * s` and `x1`, so together they start 308 units in from the right.
                let controls = if label == "Between" { 308.0 * s } else { ui::TOGGLE_W * s };
                assert!(
                    x1 - controls >= x0 + text_w + 12.0 * s,
                    "at {s}x, \"{label}\" is {:.0}px short of room and its hint would run under the control",
                    (x0 + text_w + 12.0 * s) - (x1 - controls)
                );
            }
        }
    }

    /// Why the wrapping exists. Six on one line is 309 wide and would start at 415, while the hint
    /// text runs to 432 - a seventeen-pixel collision, and worse with every character added.
    #[test]
    fn one_line_of_every_character_would_run_back_over_the_hint() {
        let f = fonts();
        let labels: Vec<&str> = Character::ALL.iter().map(|x| x.label()).collect();
        let (x0, x1) = content(1.0);
        let hint = x0 + text::width(&f.regular, 12.0, "Who your companion is — the rest stays the same");
        assert!(
            x1 - ui::segmented_width(&labels, 1.0) < hint,
            "they all fit on one line beside the hint now, so the wrapping can go"
        );
    }
}

#[cfg(test)]
mod attention_row_fits {
    use super::*;
    use pip_core::settings::Attention;

    /// Four words on one row is the widest control on the Pet page, and it was chosen over eight
    /// separate toggles. If it does not fit it has to become something else, so this is the test
    /// that would say so rather than a screenshot somebody notices later.
    ///
    /// The voice row is checked with it: a hint that grows is the usual way one of these stops
    /// fitting, and the longer wording I first tried for the preview failed this immediately.
    #[test]
    fn the_choices_fit_beside_their_own_labels() {
        let f = fonts();
        let rows: [(&str, &str, Vec<&str>); 2] = [
            (
                "Getting your attention",
                "How hard it tries when a reminder is due",
                Attention::ALL.iter().map(|a| a.label()).collect(),
            ),
            (
                "Voice",
                "Each companion has its own; tap to hear",
                Voice::ALL.iter().map(|v| v.label()).collect(),
            ),
        ];
        for s in [1.0f32, 1.25, 1.5] {
            let x0 = (SIDEBAR_W + 36.0) * s;
            let x1 = LOGICAL_W as f32 * s - 36.0 * s;
            for (label, hint, labels) in &rows {
                let lw = text::width(&f.semibold, 13.0 * s, label);
                let hw = text::width(&f.regular, 12.0 * s, hint);
                let w = ui::segmented_width(labels, s);
                let left = x0 + lw.max(hw) + 12.0 * s;
                assert!(
                    x1 - w >= left,
                    "at {s}x the {label:?} row is {:.0}px too wide and would run back over its own text",
                    left - (x1 - w)
                );
            }
        }
    }
}
