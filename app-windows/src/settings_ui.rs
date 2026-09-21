//! Settings window. Custom-drawn with the same renderer as the pet, so it matches the creature's
//! look and adds no UI framework. It exists only while open; closing frees all of it.

use crate::app::{hm, App};
use crate::platform::{self, tick_ms, Usage};
use crate::render::creature::{self, Expr, Pose};
use crate::render::text::{self, fonts};
use crate::render::ui::{self, BtnStyle, R};
use crate::render::{color, fill_rrect, Palette};
use crate::surface::Surface;
use pip_core::schedule;
use pip_core::calendar::CalendarFeed;
use pip_core::dates::{DateKind, HolidayRegion, ImportantDate};
use pip_core::settings::{CustomReminder, EventAlerts, Liveliness, PetColor, PetSize, Theme};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Pet,
    Water,
    Reminders,
    Dates,
    Calendars,
    Quiet,
    About,
}

const PAGES: [(Page, &str, &str); 7] = [
    (Page::Pet, "Companion", "Name, look and behaviour"),
    (Page::Water, "Water", "Gentle hydration reminders"),
    (Page::Reminders, "Reminders", "Things you'd like to remember"),
    (Page::Dates, "Dates", "Birthdays, anniversaries and holidays"),
    (Page::Calendars, "Calendars", "Subscribed calendars and event alerts"),
    (Page::Quiet, "Quiet & alerts", "When and how it speaks up"),
    (Page::About, "About", "Stats, privacy and resource use"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Title(String),
    DateTitle(String),
    CalName(String),
    CalUrl(String),
    Keywords,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiAct {
    Page(Page),
    Focus(Field),
    Color(PetColor),
    Size(PetSize),
    Live(Liveliness),
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
    QuietImportant,
    Sound,
    Snooze(i32),
    BubbleMin(i32),
    Fullscreen,
    AddReminder,
    RemOn(String),
    RemTime(String, i32),
    RemDay(String, u32),
    RemImportant(String),
    RemDelete(String),
    OpenData,
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
    EventLead(i32),
    AddCalendar,
    CalOn(String),
    CalImportant(String),
    CalDelete(String),
    SyncNow,
}

pub struct SettingsUi {
    pub hwnd: HWND,
    surface: Surface,
    pub page: Page,
    hits: Vec<(R, UiAct)>,
    hover: Option<R>,
    pub focus: Option<Field>,
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
        self.ui = Some(SettingsUi::new(hwnd));
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
            Page::Water => self.page_water(&mut c),
            Page::Reminders => self.page_reminders(&mut c, &focus),
            Page::Quiet => self.page_quiet(&mut c),
            Page::Dates => self.page_dates(&mut c, &focus),
            Page::Calendars => self.page_calendars(&mut c, &focus, &kw_buf),
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
            Page::Dates if self.settings.dates.items.len() < 200 => Some(("+ Add date", UiAct::AddDate)),
            Page::Calendars if self.settings.dates.calendars.len() < 10 => Some(("+ Add calendar", UiAct::AddCalendar)),
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
        creature::draw(&mut pm, &pose, pal.body, base);
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

        ui.hits = hits;
        ui.max_scroll = (content_bottom - h).max(0.0);
    }

    fn page_pet(&self, c: &mut Ctx, focus: &Option<Field>) {
        let s = c.s;
        let p = &self.settings.pet;
        let cy = c.row("Name", Some("What your companion is called"));
        let r = R::new(c.x1 - 220.0 * s, cy - 16.0 * s, 220.0 * s, 32.0 * s);
        ui::text_field(c.pm, r, &p.name, *focus == Some(Field::Name), &c.pal, s, "Pip");
        c.hits.push((r, UiAct::Focus(Field::Name)));

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
        let cy = c.row("Liveliness", Some("How often it blinks, stretches and looks around"));
        let sel = match p.liveliness { Liveliness::Calm => 0, Liveliness::Normal => 1, Liveliness::Lively => 2 };
        c.segmented(cy, &["Calm", "Normal", "Lively"], sel, vec![UiAct::Live(Liveliness::Calm), UiAct::Live(Liveliness::Normal), UiAct::Live(Liveliness::Lively)]);
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

    fn page_water(&self, c: &mut Ctx) {
        let w = &self.settings.water;
        let cy = c.row("Water reminders", Some("Gentle nudges, counted from your last glass"));
        c.toggle(cy, w.enabled, UiAct::WaterOn);
        let x1 = c.x1;
        let cy = c.row("Remind me every", None);
        c.stepper(cy, x1, 150.0, &fmt_minutes(w.interval_min), UiAct::WaterEvery(-15), UiAct::WaterEvery(15));
        let cy = c.row("Starting at", Some("No water reminders before this time"));
        c.stepper(cy, x1, 150.0, &w.start.to_string(), UiAct::WaterStart(-30), UiAct::WaterStart(30));
        let cy = c.row("Until", None);
        c.stepper(cy, x1, 150.0, &w.end.to_string(), UiAct::WaterEnd(-30), UiAct::WaterEnd(30));
        let cy = c.row("On these days", None);
        let rects = ui::day_chips(c.pm, c.x1 - ui::DAY_CHIPS_W * c.s, cy - 13.0 * c.s, w.days.0, &c.pal, c.s);
        for (i, r) in rects.into_iter().enumerate() {
            c.hits.push((r, UiAct::WaterDay(i as u32)));
        }
        let cy = c.row("Daily goal", Some("Glasses per day — for progress and celebrations"));
        let goal = if w.daily_goal == 1 { "1 glass".to_string() } else { format!("{} glasses", w.daily_goal) };
        c.stepper(cy, x1, 150.0, &goal, UiAct::WaterGoal(-1), UiAct::WaterGoal(1));
        let cy = c.row("Today", None);
        let today = local_count(self);
        let mut t = format!("{today} of {} glasses", w.daily_goal);
        if self.state.streak > 0 {
            t += &format!("  ·  {}-day streak", self.state.streak);
        }
        let f = fonts();
        let fs = 13.0 * c.s;
        let tw = text::width(&f.regular, fs, &t);
        text::draw(c.pm, &f.regular, fs, c.x1 - tw, cy + fs * 0.36, c.pal.muted, 1.0, &t);
    }

    fn page_reminders(&self, c: &mut Ctx, focus: &Option<Field>) {
        let s = c.s;
        let f = fonts();
        if self.settings.reminders.is_empty() {
            c.y += 40.0 * s;
            let msg = "No reminders yet.";
            text::draw(c.pm, &f.semibold, 15.0 * s, c.x0, c.y, c.pal.text, 1.0, msg);
            c.y += 24.0 * s;
            for line in text::wrap(&f.regular, 13.0 * s, "Add one for stand-ups, medication, stretching, calling someone — anything with a time. Mark it important and it can gently break through quiet hours.", c.x1 - c.x0) {
                text::draw(c.pm, &f.regular, 13.0 * s, c.x0, c.y, c.pal.muted, 1.0, &line);
                c.y += 19.0 * s;
            }
            return;
        }
        let now = platform::now_unix();
        for r in &self.settings.reminders {
            let card_h = 136.0 * s;
            let (x, y, w) = (c.x0 - 14.0 * s, c.y + 6.0 * s, c.x1 - c.x0 + 28.0 * s);
            fill_rrect(c.pm, x, y, w, card_h - 12.0 * s, 14.0 * s, color(c.pal.surface, 1.0));
            if r.important {
                fill_rrect(c.pm, x + 5.0 * s, y + 16.0 * s, 3.5 * s, card_h - 44.0 * s, 2.0 * s, color(0xFFB020, 1.0));
            }
            // Row 1: title + enabled
            let tr = R::new(c.x0, y + 12.0 * s, c.x1 - c.x0 - 60.0 * s, 32.0 * s);
            ui::text_field(c.pm, tr, &r.title, *focus == Some(Field::Title(r.id.clone())), &c.pal, s, "Title");
            c.hits.push((tr, UiAct::Focus(Field::Title(r.id.clone()))));
            let tg = ui::toggle(c.pm, c.x1 - ui::TOGGLE_W * s, y + 17.0 * s, r.enabled, &c.pal, s);
            c.hits.push((tg, UiAct::RemOn(r.id.clone())));
            // Row 2: time + days
            let row2 = y + 52.0 * s;
            let (m, p) = ui::stepper(c.pm, c.x0, row2, 118.0 * s, &r.time.to_string(), &c.pal, s);
            c.hits.push((m, UiAct::RemTime(r.id.clone(), -15)));
            c.hits.push((p, UiAct::RemTime(r.id.clone(), 15)));
            let chips = ui::day_chips(c.pm, c.x0 + 132.0 * s, row2 + 2.0 * s, r.days.0, &c.pal, s);
            for (i, cr) in chips.into_iter().enumerate() {
                c.hits.push((cr, UiAct::RemDay(r.id.clone(), i as u32)));
            }
            // Row 3: next occurrence + actions
            let row3 = y + 86.0 * s;
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
    }

    fn page_quiet(&self, c: &mut Ctx) {
        let q = &self.settings.quiet;
        let n = &self.settings.notify;
        let x1 = c.x1;
        let cy = c.row("Quiet hours", Some("Your pet naps and holds reminders"));
        c.toggle(cy, q.enabled, UiAct::QuietOn);
        let cy = c.row("From", None);
        c.stepper(cy, x1, 150.0, &q.start.to_string(), UiAct::QuietStart(-30), UiAct::QuietStart(30));
        let cy = c.row("Until", None);
        c.stepper(cy, x1, 150.0, &q.end.to_string(), UiAct::QuietEnd(-30), UiAct::QuietEnd(30));
        let cy = c.row("Important reminders", Some("Still shown during quiet hours, silently"));
        c.toggle(cy, q.allow_important, UiAct::QuietImportant);
        let cy = c.row("Sound", Some("A soft two-note chime with reminders"));
        c.toggle(cy, n.sound, UiAct::Sound);
        let cy = c.row("Snooze for", None);
        c.stepper(cy, x1, 150.0, &fmt_minutes(n.snooze_min), UiAct::Snooze(-5), UiAct::Snooze(5));
        let cy = c.row("Bubble stays for", Some("Then it folds into a small badge on your pet"));
        c.stepper(cy, x1, 150.0, &fmt_minutes(n.bubble_minutes), UiAct::BubbleMin(-1), UiAct::BubbleMin(1));
        let cy = c.row("Pause during fullscreen", Some("Games, videos, presentations — reminders wait"));
        c.toggle(cy, n.pause_in_fullscreen, UiAct::Fullscreen);
    }

    fn page_about(&self, c: &mut Ctx, u: Usage, cpu_rate: Option<f32>) {
        let s = c.s;
        let f = fonts();
        let now = platform::now_unix();
        let days = ((now - self.state.created_at).max(0) / 86_400) as u32;
        let (x, w) = (c.x0 - 14.0 * s, c.x1 - c.x0 + 28.0 * s);

        // Stats card
        let y = c.y + 4.0 * s;
        fill_rrect(c.pm, x, y, w, 76.0 * s, 14.0 * s, color(c.pal.surface, 1.0));
        let with = match days {
            0 => "arrived today".to_string(),
            1 => "has been with you for 1 day".to_string(),
            d => format!("has been with you for {d} days"),
        };
        text::draw(c.pm, &f.semibold, 15.0 * s, c.x0, y + 30.0 * s, c.pal.text, 1.0, &format!("{} {with}", self.settings.pet.name));
        let stats = format!(
            "Today {} of {} glasses  ·  streak {} (best {})",
            local_count(self),
            self.settings.water.daily_goal,
            self.state.streak,
            self.state.best_streak
        );
        text::draw(c.pm, &f.regular, 12.5 * s, c.x0, y + 54.0 * s, c.pal.muted, 1.0, &stats);
        c.y = y + 92.0 * s;

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
        let privacy = "Everything stays on this computer: no account, no network access, no tracking. Your settings live in a small folder you can open below.";
        for l in text::wrap(&f.regular, 12.5 * s, privacy, c.x1 - c.x0) {
            text::draw(c.pm, &f.regular, 12.5 * s, c.x0, c.y + 14.0 * s, c.pal.muted, 1.0, &l);
            c.y += 19.0 * s;
        }
        c.y += 14.0 * s;
        let mut bx = c.x0;
        let y = c.y;
        bx += c.button(bx, y, "Open data folder", BtnStyle::Secondary, UiAct::OpenData) + 8.0 * s;
        bx += c.button(bx, y, "Reset position", BtnStyle::Secondary, UiAct::ResetPos) + 8.0 * s;
        c.button(bx, y, "Test a reminder", BtnStyle::Secondary, UiAct::TestReminder);
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
        let was_focus = ui.focus.clone();
        // Leaving a text field (for anything but the same field) commits it.
        if was_focus.is_some() && act != was_focus.clone().map(UiAct::Focus) {
            if let Some(ui) = &mut self.ui {
                ui.focus = None;
            }
            self.ui_commit_text();
        }
        match act {
            Some(UiAct::Focus(f)) => {
                let keywords = self.settings.dates.keywords.join(", ");
                if let Some(ui) = &mut self.ui {
                    if f == Field::Keywords && ui.focus != Some(Field::Keywords) {
                        ui.keywords_buf = keywords;
                    }
                    ui.focus = Some(f);
                }
            }
            Some(a) => self.ui_do(a),
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

    fn ui_commit_text(&mut self) {
        // Diff against the settings as they were when editing began, so e.g. a changed calendar
        // link is noticed (and re-encrypted + synced) even though edits were applied live.
        let prev = self.ui.as_mut().and_then(|u| u.edit_origin.take()).map(|b| *b).unwrap_or_else(|| self.settings.clone());
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
            Field::Keywords => {
                if let Some(ui) = &mut self.ui {
                    edit(&mut ui.keywords_buf, 300);
                    self.settings.dates.keywords =
                        ui.keywords_buf.split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect();
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
        if self.ui_do_dates(&act) {
            if prev != self.settings {
                self.settings_changed(&prev);
            }
            return;
        }
        let now = platform::now_unix();
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
                }
                return;
            }
            UiAct::Focus(_) => return,
            UiAct::Color(c) => st.pet.color = c,
            UiAct::Size(z) => st.pet.size = z,
            UiAct::Live(l) => st.pet.liveliness = l,
            UiAct::Theme(t) => st.pet.theme = t,
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
            UiAct::QuietImportant => st.quiet.allow_important = !st.quiet.allow_important,
            UiAct::Sound => st.notify.sound = !st.notify.sound,
            UiAct::Snooze(d) => st.notify.snooze_min = (st.notify.snooze_min as i32 + d).clamp(5, 60) as u32,
            UiAct::BubbleMin(d) => st.notify.bubble_minutes = (st.notify.bubble_minutes as i32 + d).clamp(1, 30) as u32,
            UiAct::Fullscreen => st.notify.pause_in_fullscreen = !st.notify.pause_in_fullscreen,
            UiAct::AddReminder => {
                let next_hour = TimeOfDay::of(pip_core::time::local(&chrono::Local, now + 3600)).0 / 60 * 60;
                let id = format!("r{:x}{:04x}", now, (platform::tick_ms() & 0xffff));
                st.reminders.push(CustomReminder {
                    id: id.clone(),
                    title: String::new(), // placeholder shows; empty becomes "Reminder" on commit
                    time: TimeOfDay(next_hour),
                    days: DaySet::WEEKDAYS,
                    updated_at: now,
                    ..Default::default()
                });
                if let Some(ui) = &mut self.ui {
                    ui.focus = Some(Field::Title(id));
                    ui.scroll = f32::MAX; // clamped on next paint → shows the new card
                }
            }
            UiAct::RemOn(id) => {
                if let Some(r) = st.reminder_mut(&id) {
                    r.enabled = !r.enabled;
                    r.updated_at = now;
                    if r.enabled && r.days.is_empty() {
                        r.date = Some(schedule::next_date_for(r.time, now, &chrono::Local));
                    }
                }
            }
            UiAct::RemTime(id, d) => {
                if let Some(r) = st.reminder_mut(&id) {
                    r.time = r.time.add_minutes(d);
                    r.updated_at = now;
                    if r.days.is_empty() {
                        r.date = Some(schedule::next_date_for(r.time, now, &chrono::Local));
                    }
                }
            }
            UiAct::RemDay(id, i) => {
                if let Some(r) = st.reminder_mut(&id) {
                    r.days.toggle(i);
                    r.updated_at = now;
                    // No days = a one-off at the next occurrence of its time.
                    r.date = r.days.is_empty().then(|| schedule::next_date_for(r.time, now, &chrono::Local));
                }
            }
            UiAct::RemImportant(id) => {
                if let Some(r) = st.reminder_mut(&id) {
                    r.important = !r.important;
                    r.updated_at = now;
                }
            }
            UiAct::RemDelete(id) => st.reminders.retain(|r| r.id != id),
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
                    platform::Chime::new().play();
                }
                return;
            }
            _ => return, // dates/calendar actions are handled by ui_do_dates
        }
        if prev != self.settings {
            self.settings_changed(&prev);
            if prev.pet.color != self.settings.pet.color {
                if let Some(ui) = &self.ui {
                    set_window_icon(ui.hwnd, self.settings.pet.color.rgb());
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

pub fn set_window_icon(hwnd: HWND, body: u32) {
    unsafe {
        for (which, size) in [(ICON_SMALL, 16 * 2), (ICON_BIG, 32 * 2)] {
            let icon = crate::tray::make_icon(body, false, size);
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

    fn page_dates(&self, c: &mut Ctx, focus: &Option<Field>) {
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
        let d = &self.settings.dates;
        let x1 = c.x1;
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
        let cy = c.row("Alert me", Some("For events without an alert of their own"));
        let lead = if d.event_lead_min == 0 { "At the start".to_string() } else { format!("{} min before", d.event_lead_min) };
        c.stepper(cy, x1, 160.0, &lead, UiAct::EventLead(-5), UiAct::EventLead(5));
        let cy = c.row("Important words", Some("Comma-separated, e.g. interview, dentist"));
        let editing = *focus == Some(Field::Keywords);
        let value = if editing { kw_buf.to_string() } else { d.keywords.join(", ") };
        let r = R::new(c.x1 - 250.0 * s, cy - 16.0 * s, 250.0 * s, 32.0 * s);
        ui::text_field(c.pm, r, &value, editing, &c.pal, s, "No keywords");
        c.hits.push((r, UiAct::Focus(Field::Keywords)));
        let last = d.calendars.iter().filter_map(|c| self.feed_meta.get(&c.id)).map(|m| m.synced_at).max().unwrap_or(0);
        let hint = if d.calendars.is_empty() {
            "Add a calendar to start".to_string()
        } else if last > 0 {
            format!("Last synced {}. Checks hourly.", ago(now - last))
        } else {
            "Not synced yet".to_string()
        };
        let cy = c.row("Sync", Some(&hint));
        if !d.calendars.is_empty() {
            let label = "Sync now";
            let bw = ui::button_width(label, s);
            c.button(x1 - bw, cy - 15.0 * s, label, BtnStyle::Secondary, UiAct::SyncNow);
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
            let ur = R::new(c.x0, y + 52.0 * s, c.x1 - c.x0, 32.0 * s);
            let editing_url = *focus == Some(Field::CalUrl(id.clone()));
            let shown = if editing_url { feed.url.clone() } else { masked_url(&feed.url) };
            ui::text_field(c.pm, ur, &shown, editing_url, &c.pal, s, "Paste the calendar's private iCal / webcal link");
            c.hits.push((ur, UiAct::Focus(Field::CalUrl(id.clone()))));
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
        let help = [
            "Where to find a link:",
            "Google Calendar — Settings › your calendar › Integrate calendar › Secret address in iCal format.",
            "Outlook — Settings › Calendar › Shared calendars › Publish a calendar › ICS link.",
            "iCloud — Calendar app › share icon › Public calendar › copy link.",
            "Links are stored encrypted for your Windows account. Pip only reads them.",
        ];
        for (i, h) in help.iter().enumerate() {
            let font = if i == 0 { &f.semibold } else { &f.regular };
            for line in text::wrap(font, 12.0 * s, h, c.x1 - c.x0) {
                c.y += 18.0 * s;
                text::draw(c.pm, font, 12.0 * s, c.x0, c.y, c.pal.muted, 1.0, &line);
            }
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
            UiAct::EventLead(m) => d.event_lead_min = (d.event_lead_min as i32 + m).clamp(0, 60) as u32,
            UiAct::AddCalendar => {
                let id = new_id("c");
                d.calendars.push(CalendarFeed { id: id.clone(), updated_at: now, ..Default::default() });
                if let Some(ui) = &mut self.ui {
                    ui.focus = Some(Field::CalUrl(id));
                    ui.scroll = f32::MAX;
                }
            }
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
