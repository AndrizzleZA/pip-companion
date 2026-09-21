//! iCalendar (RFC 5545) subscription support: parse `.ics` feeds and expand recurring events.
//!
//! Deliberately a focused subset — the parts real feeds from Google, Outlook and iCloud use:
//! VEVENT with DTSTART/DTEND/DURATION, all-day events, RRULE (DAILY/WEEKLY/MONTHLY/YEARLY with
//! INTERVAL, COUNT, UNTIL, BYDAY incl. ordinals, BYMONTHDAY, BYMONTH), EXDATE, RECURRENCE-ID
//! overrides, STATUS:CANCELLED, VALARM triggers, and VTIMEZONE with yearly DST rules.
//! Anything unsupported degrades gracefully to "the first occurrence only".

use crate::time::{local, to_ts};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A subscribed calendar. The URL is a secret (it grants read access) — shells store it
/// encrypted and never in the plain settings file, hence `skip`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarFeed {
    pub id: String,
    pub name: String,
    #[serde(skip)]
    pub url: String,
    pub enabled: bool,
    /// Every timed event from this calendar is important (e.g. a work calendar).
    pub important: bool,
    pub updated_at: i64,
}

impl Default for CalendarFeed {
    fn default() -> Self {
        CalendarFeed { id: String::new(), name: "Calendar".into(), url: String::new(), enabled: true, important: false, updated_at: 0 }
    }
}

/// One concrete occurrence of an event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub uid: String,
    pub title: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    /// Minutes before start from the event's own VALARM, if it has one.
    pub alarm_min: Option<i64>,
    pub feed_id: String,
}

impl Event {
    /// Stable key for acknowledging this occurrence.
    pub fn key(&self) -> String {
        format!("{}@{}", self.uid, self.start)
    }
}

// ---------------------------------------------------------------------------------------------
// Parsed model
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Zone {
    Utc,
    Floating,
    Named(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Dt {
    t: NaiveDateTime,
    zone: Zone,
    date_only: bool,
}

#[derive(Clone, Debug, Default)]
struct TzDef {
    std_offset: i32,
    dst: Option<(i32, YearRule, YearRule)>, // (dst offset, dst starts, standard starts)
}

#[derive(Clone, Copy, Debug)]
struct YearRule {
    month: u32,
    ord: i32,
    wd: Weekday,
    time: NaiveTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Debug)]
struct RRule {
    freq: Freq,
    interval: i64,
    count: Option<u32>,
    until: Option<Dt>,
    byday: Vec<(i32, Weekday)>, // ordinal 0 = every
    bymonthday: Vec<i32>,
    bymonth: Vec<u32>,
}

#[derive(Clone, Debug)]
struct VEvent {
    uid: String,
    summary: String,
    start: Dt,
    end: Option<Dt>,
    duration: Option<i64>,
    rrule: Option<RRule>,
    exdates: Vec<Dt>,
    recurrence_id: Option<Dt>,
    cancelled: bool,
    alarm_min: Option<i64>,
}

pub struct ParsedCalendar {
    events: Vec<VEvent>,
    zones: Vec<TzDef>,
    pub name: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------------

struct Line<'a> {
    name: String,
    params: Vec<(String, String)>,
    value: &'a str,
}

impl Line<'_> {
    fn param(&self, key: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

fn unfold(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if (raw.starts_with(' ') || raw.starts_with('\t')) && !out.is_empty() {
            out.last_mut().unwrap().push_str(&raw[1..]);
        } else if !raw.is_empty() {
            out.push(raw.to_string());
        }
    }
    out
}

fn parse_line(s: &str) -> Option<Line<'_>> {
    // The value starts at the first ':' outside double quotes.
    let mut in_q = false;
    let mut colon = None;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_q = !in_q,
            ':' if !in_q => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&s[..colon], &s[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.trim().to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('=').map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim().trim_matches('"').to_string())))
        .collect();
    Some(Line { name, params, value })
}

fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_date_time(v: &str) -> Option<(NaiveDateTime, bool, bool)> {
    let v = v.trim();
    let (body, utc) = match v.strip_suffix('Z') {
        Some(b) => (b, true),
        None => (v, false),
    };
    if body.len() == 8 {
        let d = NaiveDate::parse_from_str(body, "%Y%m%d").ok()?;
        return Some((d.and_hms_opt(0, 0, 0)?, false, true));
    }
    let t = NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M%S").ok()?;
    Some((t, utc, false))
}

/// RFC 5545 duration → seconds (e.g. "-PT15M", "P1D", "PT1H30M", "P2W").
pub fn parse_duration(v: &str) -> Option<i64> {
    let v = v.trim();
    let (sign, rest) = match v.as_bytes().first()? {
        b'-' => (-1, &v[1..]),
        b'+' => (1, &v[1..]),
        _ => (1, v),
    };
    let rest = rest.strip_prefix('P')?;
    let mut total = 0i64;
    let mut num = String::new();
    let mut in_time = false;
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            _ => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += n * match (c, in_time) {
                    ('W', _) => 604_800,
                    ('D', _) => 86_400,
                    ('H', true) => 3_600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
        }
    }
    Some(sign * total)
}

fn weekday(s: &str) -> Option<Weekday> {
    Some(match s {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

fn parse_byday(v: &str) -> Vec<(i32, Weekday)> {
    v.split(',')
        .filter_map(|p| {
            let p = p.trim();
            let (ord, wd) = p.split_at(p.len().saturating_sub(2));
            Some((if ord.is_empty() { 0 } else { ord.parse().ok()? }, weekday(wd)?))
        })
        .collect()
}

fn parse_offset(v: &str) -> Option<i32> {
    let v = v.trim();
    let sign = if v.starts_with('-') { -1 } else { 1 };
    let digits = v.trim_start_matches(['+', '-']);
    if digits.len() < 4 {
        return None;
    }
    let h: i32 = digits[0..2].parse().ok()?;
    let m: i32 = digits[2..4].parse().ok()?;
    Some(sign * (h * 3600 + m * 60))
}

/// Offsets for time zones that feeds reference without a VTIMEZONE block. Kept tiny on purpose.
fn well_known_zone(tzid: &str) -> Option<TzDef> {
    let fixed = |h: i32| Some(TzDef { std_offset: h * 3600, dst: None });
    match tzid {
        "UTC" | "Etc/UTC" | "GMT" | "Z" => fixed(0),
        "Africa/Johannesburg" | "South Africa Standard Time" | "Africa/Harare" | "Africa/Maputo" => fixed(2),
        _ => None,
    }
}

pub fn parse_ics(text: &str) -> ParsedCalendar {
    let lines = unfold(text);
    let mut zones: Vec<TzDef> = Vec::new();
    let mut zone_ids: HashMap<String, usize> = HashMap::new();

    // Pass 1: VTIMEZONE definitions (standard + optional daylight sub-blocks with yearly rules).
    {
        struct Sub {
            daylight: bool,
            offset: i32,
            rule: Option<YearRule>,
            start: Option<NaiveDateTime>,
        }
        let mut tzid: Option<String> = None;
        let mut subs: Vec<Sub> = Vec::new();
        let mut sub: Option<Sub> = None;
        for raw in &lines {
            let Some(l) = parse_line(raw) else { continue };
            match (l.name.as_str(), l.value.trim()) {
                ("BEGIN", "VTIMEZONE") => {
                    tzid = Some(String::new());
                    subs.clear();
                }
                ("BEGIN", kind @ ("STANDARD" | "DAYLIGHT")) if tzid.is_some() => {
                    sub = Some(Sub { daylight: kind == "DAYLIGHT", offset: 0, rule: None, start: None })
                }
                ("END", "STANDARD" | "DAYLIGHT") => subs.extend(sub.take()),
                ("END", "VTIMEZONE") => {
                    let Some(id) = tzid.take() else { continue };
                    // Rule time-of-day comes from the sub-block's DTSTART.
                    let rule_of = |s: &Sub| s.rule.map(|r| YearRule { time: s.start.map(|t| t.time()).unwrap_or(r.time), ..r });
                    let std = subs.iter().rev().find(|s| !s.daylight);
                    let dst = subs.iter().rev().find(|s| s.daylight);
                    let mut def = TzDef { std_offset: std.or(dst).map_or(0, |s| s.offset), dst: None };
                    if let (Some(d), Some(s)) = (dst, std) {
                        if let (Some(dr), Some(sr)) = (rule_of(d), rule_of(s)) {
                            def.dst = Some((d.offset, dr, sr));
                        }
                    }
                    zone_ids.insert(id, zones.len());
                    zones.push(def);
                }
                ("TZID", v) if tzid.is_some() && sub.is_none() => tzid = Some(v.to_string()),
                ("TZOFFSETTO", v) => {
                    if let Some(s) = &mut sub {
                        s.offset = parse_offset(v).unwrap_or(0);
                    }
                }
                ("DTSTART", v) => {
                    if let Some(s) = &mut sub {
                        s.start = parse_date_time(v).map(|x| x.0);
                    }
                }
                ("RRULE", v) => {
                    let Some(s) = &mut sub else { continue };
                    let mut month = 0;
                    let mut bd = Vec::new();
                    for kv in v.split(';') {
                        match kv.split_once('=') {
                            Some(("BYMONTH", m)) => month = m.parse().unwrap_or(0),
                            Some(("BYDAY", d)) => bd = parse_byday(d),
                            _ => {}
                        }
                    }
                    if let (true, Some(&(ord, wd))) = (month > 0, bd.first()) {
                        s.rule = Some(YearRule { month, ord: if ord == 0 { 1 } else { ord }, wd, time: NaiveTime::MIN });
                    }
                }
                _ => {}
            }
        }
    }

    let mut zone_of = |params_tzid: Option<&str>| -> Zone {
        match params_tzid {
            None => Zone::Floating,
            Some(id) => {
                if let Some(&i) = zone_ids.get(id) {
                    Zone::Named(i)
                } else if let Some(def) = well_known_zone(id) {
                    zone_ids.insert(id.to_string(), zones.len());
                    zones.push(def);
                    Zone::Named(zones.len() - 1)
                } else {
                    Zone::Floating // unknown zone: treat as local wall-clock time
                }
            }
        }
    };

    // Pass 2: events.
    let mut events = Vec::new();
    let mut name = None;
    let mut cur: Option<VEvent> = None;
    let mut in_alarm = false;
    for raw in &lines {
        let Some(l) = parse_line(raw) else { continue };
        let dt = |l: &Line, zone_of: &mut dyn FnMut(Option<&str>) -> Zone| -> Option<Dt> {
            let (t, utc, date_only) = parse_date_time(l.value)?;
            let zone = if utc { Zone::Utc } else if date_only { Zone::Floating } else { zone_of(l.param("TZID")) };
            Some(Dt { t, zone, date_only: date_only || l.param("VALUE") == Some("DATE") })
        };
        match l.name.as_str() {
            "X-WR-CALNAME" if cur.is_none() => name = Some(unescape(l.value)),
            "BEGIN" if l.value.trim() == "VEVENT" => {
                cur = Some(VEvent {
                    uid: String::new(),
                    summary: String::new(),
                    start: Dt { t: NaiveDateTime::MIN, zone: Zone::Floating, date_only: false },
                    end: None,
                    duration: None,
                    rrule: None,
                    exdates: Vec::new(),
                    recurrence_id: None,
                    cancelled: false,
                    alarm_min: None,
                })
            }
            "BEGIN" if l.value.trim() == "VALARM" => in_alarm = true,
            "END" if l.value.trim() == "VALARM" => in_alarm = false,
            "END" if l.value.trim() == "VEVENT" => {
                if let Some(e) = cur.take() {
                    if e.start.t != NaiveDateTime::MIN {
                        events.push(e);
                    }
                }
            }
            _ => {
                let Some(e) = &mut cur else { continue };
                if in_alarm {
                    if l.name == "TRIGGER" && l.param("VALUE") != Some("DATE-TIME") {
                        if let Some(secs) = parse_duration(l.value) {
                            let before = (-secs / 60).max(0);
                            e.alarm_min = Some(e.alarm_min.map_or(before, |m| m.max(before)));
                        }
                    }
                    continue;
                }
                match l.name.as_str() {
                    "UID" => e.uid = l.value.trim().to_string(),
                    "SUMMARY" => e.summary = unescape(l.value),
                    "DTSTART" => {
                        if let Some(d) = dt(&l, &mut zone_of) {
                            e.start = d
                        }
                    }
                    "DTEND" => e.end = dt(&l, &mut zone_of),
                    "DURATION" => e.duration = parse_duration(l.value),
                    "RECURRENCE-ID" => e.recurrence_id = dt(&l, &mut zone_of),
                    "STATUS" => e.cancelled = l.value.trim().eq_ignore_ascii_case("CANCELLED"),
                    "EXDATE" => {
                        let zone = l.param("TZID");
                        for part in l.value.split(',') {
                            if let Some((t, utc, date_only)) = parse_date_time(part) {
                                let z = if utc { Zone::Utc } else if date_only { Zone::Floating } else { zone_of(zone) };
                                e.exdates.push(Dt { t, zone: z, date_only });
                            }
                        }
                    }
                    "RRULE" => e.rrule = parse_rrule(l.value, &mut zone_of),
                    _ => {}
                }
            }
        }
    }
    ParsedCalendar { events, zones, name }
}

fn parse_rrule(v: &str, zone_of: &mut dyn FnMut(Option<&str>) -> Zone) -> Option<RRule> {
    let mut r = RRule { freq: Freq::Daily, interval: 1, count: None, until: None, byday: vec![], bymonthday: vec![], bymonth: vec![] };
    let mut freq = None;
    for kv in v.trim().split(';') {
        let Some((k, val)) = kv.split_once('=') else { continue };
        match k.to_ascii_uppercase().as_str() {
            "FREQ" => {
                freq = Some(match val {
                    "DAILY" => Freq::Daily,
                    "WEEKLY" => Freq::Weekly,
                    "MONTHLY" => Freq::Monthly,
                    "YEARLY" => Freq::Yearly,
                    _ => return None, // HOURLY etc: not supported
                })
            }
            "INTERVAL" => r.interval = val.parse().unwrap_or(1).max(1),
            "COUNT" => r.count = val.parse().ok(),
            "UNTIL" => {
                r.until = parse_date_time(val).map(|(t, utc, d)| Dt { t, zone: if utc { Zone::Utc } else { zone_of(None) }, date_only: d })
            }
            "BYDAY" => r.byday = parse_byday(val),
            "BYMONTHDAY" => r.bymonthday = val.split(',').filter_map(|x| x.parse().ok()).collect(),
            "BYMONTH" => r.bymonth = val.split(',').filter_map(|x| x.parse().ok()).collect(),
            _ => {}
        }
    }
    r.freq = freq?;
    Some(r)
}

// ---------------------------------------------------------------------------------------------
// Time zone resolution
// ---------------------------------------------------------------------------------------------

/// The date of the `ord`-th `wd` in a month (ord < 0 counts from the end).
fn nth_weekday(year: i32, month: u32, ord: i32, wd: Weekday) -> Option<NaiveDate> {
    if ord > 0 {
        let first = NaiveDate::from_ymd_opt(year, month, 1)?;
        let shift = (7 + wd.num_days_from_monday() as i64 - first.weekday().num_days_from_monday() as i64) % 7;
        let d = first + Duration::days(shift + 7 * (ord as i64 - 1));
        (d.month() == month).then_some(d)
    } else {
        let next_month = if month == 12 { NaiveDate::from_ymd_opt(year + 1, 1, 1)? } else { NaiveDate::from_ymd_opt(year, month + 1, 1)? };
        let last = next_month.pred_opt()?;
        let shift = (7 + last.weekday().num_days_from_monday() as i64 - wd.num_days_from_monday() as i64) % 7;
        let d = last - Duration::days(shift + 7 * (-ord as i64 - 1));
        (d.month() == month).then_some(d)
    }
}

impl TzDef {
    fn offset_at(&self, t: NaiveDateTime) -> i32 {
        let Some((dst_off, dst_rule, std_rule)) = self.dst else { return self.std_offset };
        let y = t.year();
        let at = |r: YearRule| nth_weekday(y, r.month, r.ord, r.wd).map(|d| d.and_time(r.time));
        match (at(dst_rule), at(std_rule)) {
            (Some(ds), Some(ss)) => {
                let in_dst = if ds < ss { t >= ds && t < ss } else { t >= ds || t < ss }; // southern hemisphere wraps
                if in_dst { dst_off } else { self.std_offset }
            }
            _ => self.std_offset,
        }
    }
}

impl ParsedCalendar {
    fn resolve<Tz: TimeZone>(&self, d: &Dt, tz: &Tz) -> i64 {
        match d.zone {
            Zone::Utc => d.t.and_utc().timestamp(),
            Zone::Floating => to_ts(tz, d.t),
            Zone::Named(i) => d.t.and_utc().timestamp() - self.zones[i].offset_at(d.t) as i64,
        }
    }

    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    /// All occurrences overlapping [from, to).
    pub fn expand<Tz: TimeZone>(&self, feed_id: &str, from: i64, to: i64, tz: &Tz) -> Vec<Event> {
        let mut out = Vec::new();
        // Occurrences replaced by RECURRENCE-ID overrides.
        let overridden: HashSet<(String, i64)> = self
            .events
            .iter()
            .filter_map(|e| e.recurrence_id.as_ref().map(|r| (e.uid.clone(), self.resolve(r, tz))))
            .collect();

        for e in &self.events {
            if e.cancelled && e.recurrence_id.is_none() {
                continue;
            }
            let all_day = e.start.date_only;
            let start_ts = self.resolve(&e.start, tz);
            let dur = match (&e.end, e.duration) {
                (Some(end), _) => (self.resolve(end, tz) - start_ts).max(0),
                (None, Some(d)) => d.max(0),
                (None, None) => if all_day { 86_400 } else { 0 },
            };
            let make = |s: i64| Event {
                uid: e.uid.clone(),
                title: if e.summary.trim().is_empty() { "(No title)".into() } else { e.summary.trim().to_string() },
                start: s,
                end: s + dur,
                all_day,
                alarm_min: e.alarm_min,
                feed_id: feed_id.to_string(),
            };
            if e.recurrence_id.is_some() {
                if !e.cancelled && start_ts < to && start_ts + dur.max(1) > from {
                    out.push(make(start_ts));
                }
                continue;
            }
            let Some(rule) = &e.rrule else {
                if start_ts < to && start_ts + dur.max(1) > from {
                    out.push(make(start_ts));
                }
                continue;
            };
            let exdates: HashSet<i64> = e.exdates.iter().map(|x| self.resolve(x, tz)).collect();
            let until = rule.until.as_ref().map(|u| if u.date_only { self.resolve(u, tz) + 86_399 } else { self.resolve(u, tz) });
            let mut produced = 0u32;
            for local_t in instances(&e.start, rule, 4000) {
                let dt = Dt { t: local_t, zone: e.start.zone, date_only: all_day };
                let s = self.resolve(&dt, tz);
                if until.is_some_and(|u| s > u) || rule.count.is_some_and(|c| produced >= c) || s >= to {
                    break;
                }
                produced += 1;
                if s + dur.max(1) <= from || exdates.contains(&s) || overridden.contains(&(e.uid.clone(), s)) {
                    continue;
                }
                out.push(make(s));
            }
        }
        out.sort_by_key(|e| e.start);
        out
    }
}

/// Candidate start times in the event's own wall-clock frame, in order, starting at DTSTART.
fn instances(start: &Dt, r: &RRule, cap: usize) -> Vec<NaiveDateTime> {
    let t0 = start.t;
    let time = t0.time();
    let d0 = t0.date();
    let mut out = Vec::new();
    let push = |d: NaiveDate, out: &mut Vec<NaiveDateTime>| {
        let t = d.and_time(time);
        if t >= t0 {
            out.push(t);
        }
    };
    let months_from = |k: i64| -> (i32, u32) {
        let total = d0.year() as i64 * 12 + d0.month0() as i64 + k;
        ((total / 12) as i32, (total % 12) as u32 + 1)
    };
    let mut k: i64 = 0;
    while out.len() < cap && k < cap as i64 * 4 {
        match r.freq {
            Freq::Daily => push(d0 + Duration::days(k * r.interval), &mut out),
            Freq::Weekly => {
                let week_start = d0 - Duration::days(d0.weekday().num_days_from_monday() as i64) + Duration::weeks(k * r.interval);
                let mut days: Vec<Weekday> = r.byday.iter().map(|(_, w)| *w).collect();
                if days.is_empty() {
                    days.push(d0.weekday());
                }
                days.sort_by_key(|w| w.num_days_from_monday());
                for w in days {
                    push(week_start + Duration::days(w.num_days_from_monday() as i64), &mut out);
                }
            }
            Freq::Monthly => {
                let (y, m) = months_from(k * r.interval);
                let mut days: Vec<NaiveDate> = Vec::new();
                if !r.bymonthday.is_empty() {
                    for &md in &r.bymonthday {
                        let d = if md > 0 {
                            NaiveDate::from_ymd_opt(y, m, md as u32)
                        } else {
                            last_day(y, m).and_then(|l| l.checked_sub_signed(Duration::days((-md - 1) as i64)))
                        };
                        days.extend(d);
                    }
                } else if !r.byday.is_empty() {
                    for &(ord, wd) in &r.byday {
                        if ord != 0 {
                            days.extend(nth_weekday(y, m, ord, wd));
                        } else {
                            (1..=5).filter_map(|n| nth_weekday(y, m, n, wd)).for_each(|d| days.push(d));
                        }
                    }
                } else {
                    days.extend(NaiveDate::from_ymd_opt(y, m, d0.day())); // 31st skips short months
                }
                days.sort();
                days.into_iter().for_each(|d| push(d, &mut out));
            }
            Freq::Yearly => {
                let y = d0.year() + (k * r.interval) as i32;
                let months = if r.bymonth.is_empty() { vec![d0.month()] } else { r.bymonth.clone() };
                let mut days = Vec::new();
                for m in months {
                    if let Some(&(ord, wd)) = r.byday.iter().find(|(o, _)| *o != 0) {
                        days.extend(nth_weekday(y, m, ord, wd));
                    } else if !r.bymonthday.is_empty() {
                        days.extend(r.bymonthday.iter().filter(|x| **x > 0).filter_map(|&md| NaiveDate::from_ymd_opt(y, m, md as u32)));
                    } else {
                        days.extend(NaiveDate::from_ymd_opt(y, m, d0.day()));
                    }
                }
                days.sort();
                days.into_iter().for_each(|d| push(d, &mut out));
            }
        }
        k += 1;
    }
    out.truncate(cap);
    out
}

fn last_day(y: i32, m: u32) -> Option<NaiveDate> {
    let next = if m == 12 { NaiveDate::from_ymd_opt(y + 1, 1, 1)? } else { NaiveDate::from_ymd_opt(y, m + 1, 1)? };
    next.pred_opt()
}

/// Convenience for shells: occurrences for local days [from_day, from_day + days).
pub fn expand_days<Tz: TimeZone>(cal: &ParsedCalendar, feed_id: &str, from_day: NaiveDate, days: i64, tz: &Tz) -> Vec<Event> {
    let from = to_ts(tz, from_day.and_hms_opt(0, 0, 0).unwrap());
    let to = to_ts(tz, (from_day + Duration::days(days)).and_hms_opt(0, 0, 0).unwrap());
    cal.expand(feed_id, from, to, tz)
}

/// Local date an event starts on (all-day events keep their calendar date).
pub fn event_day<Tz: TimeZone>(e: &Event, tz: &Tz) -> NaiveDate {
    local(tz, e.start).date()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(7200).unwrap()
    }
    fn ts(y: i32, m: u32, d: u32, h: u32, mi: u32) -> i64 {
        to_ts(&tz(), NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, mi, 0).unwrap())
    }
    fn wrap(body: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Test\r\n{body}END:VCALENDAR\r\n")
    }

    #[test]
    fn basics_utc_allday_folding_escapes() {
        let ics = wrap(
            "BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Dentist\\, Dr Smit\r\nDTSTART:20260922T070000Z\r\nDTEND:20260922T080000Z\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:b\r\nSUMMARY:Long\r\n  folded title\r\nDTSTART;VALUE=DATE:20260924\r\nDTEND;VALUE=DATE:20260925\r\nEND:VEVENT\r\n",
        );
        let cal = parse_ics(&ics);
        assert_eq!(cal.name.as_deref(), Some("Test"));
        let ev = cal.expand("f", ts(2026, 9, 21, 0, 0), ts(2026, 9, 30, 0, 0), &tz());
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].title, "Dentist, Dr Smit");
        assert_eq!(ev[0].start, ts(2026, 9, 22, 9, 0)); // 07:00Z = 09:00 SAST
        assert!(ev[1].all_day);
        assert_eq!(ev[1].title, "Long folded title");
        assert_eq!(ev[1].start, ts(2026, 9, 24, 0, 0));
    }

    #[test]
    fn weekly_byday_count_exdate_override() {
        let ics = wrap(
            "BEGIN:VEVENT\r\nUID:s\r\nSUMMARY:Stand-up\r\nDTSTART;TZID=Africa/Johannesburg:20260921T093000\r\nDURATION:PT15M\r\n\
             RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR;COUNT=6\r\nEXDATE;TZID=Africa/Johannesburg:20260923T093000\r\n\
             BEGIN:VALARM\r\nTRIGGER:-PT10M\r\nACTION:DISPLAY\r\nEND:VALARM\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:s\r\nRECURRENCE-ID;TZID=Africa/Johannesburg:20260925T093000\r\nSUMMARY:Stand-up (moved)\r\n\
             DTSTART;TZID=Africa/Johannesburg:20260925T110000\r\nDTEND;TZID=Africa/Johannesburg:20260925T111500\r\nEND:VEVENT\r\n",
        );
        let ev = parse_ics(&ics).expand("f", ts(2026, 9, 20, 0, 0), ts(2026, 10, 30, 0, 0), &tz());
        let starts: Vec<i64> = ev.iter().map(|e| e.start).collect();
        // Mon 21, (Wed 23 excluded), Fri 25 moved to 11:00, Mon 28, Wed 30, Fri 2 Oct = COUNT 6 generated.
        assert_eq!(
            starts,
            vec![ts(2026, 9, 21, 9, 30), ts(2026, 9, 25, 11, 0), ts(2026, 9, 28, 9, 30), ts(2026, 9, 30, 9, 30), ts(2026, 10, 2, 9, 30)]
        );
        assert_eq!(ev[0].alarm_min, Some(10));
        assert_eq!(ev[0].end - ev[0].start, 15 * 60);
        assert_eq!(ev[1].title, "Stand-up (moved)");
    }

    #[test]
    fn monthly_last_friday_and_yearly_until_and_cancelled() {
        let ics = wrap(
            "BEGIN:VEVENT\r\nUID:m\r\nSUMMARY:Payday drinks\r\nDTSTART:20260925T170000\r\nRRULE:FREQ=MONTHLY;BYDAY=-1FR\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:y\r\nSUMMARY:Renew licence\r\nDTSTART;VALUE=DATE:20261001\r\nRRULE:FREQ=YEARLY;UNTIL=20271231\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:c\r\nSUMMARY:Gone\r\nSTATUS:CANCELLED\r\nDTSTART:20260926T100000\r\nEND:VEVENT\r\n",
        );
        let cal = parse_ics(&ics);
        let ev = cal.expand("f", ts(2026, 9, 1, 0, 0), ts(2029, 1, 1, 0, 0), &tz());
        let paydays: Vec<i64> = ev.iter().filter(|e| e.uid == "m").take(3).map(|e| e.start).collect();
        assert_eq!(paydays, vec![ts(2026, 9, 25, 17, 0), ts(2026, 10, 30, 17, 0), ts(2026, 11, 27, 17, 0)]);
        assert_eq!(ev.iter().filter(|e| e.uid == "y").count(), 2); // 2026 and 2027 only
        assert!(!ev.iter().any(|e| e.uid == "c"));
    }

    #[test]
    fn vtimezone_with_dst_rules() {
        // Europe/London style: BST (+1) from last Sunday of March to last Sunday of October.
        let ics = wrap(
            "BEGIN:VTIMEZONE\r\nTZID:Europe/London\r\n\
             BEGIN:DAYLIGHT\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\nDTSTART:19810329T010000\r\nRRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU\r\nEND:DAYLIGHT\r\n\
             BEGIN:STANDARD\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0000\r\nDTSTART:19961027T020000\r\nRRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU\r\nEND:STANDARD\r\n\
             END:VTIMEZONE\r\n\
             BEGIN:VEVENT\r\nUID:l\r\nSUMMARY:London call\r\nDTSTART;TZID=Europe/London:20260915T140000\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:w\r\nSUMMARY:Winter call\r\nDTSTART;TZID=Europe/London:20261215T140000\r\nEND:VEVENT\r\n",
        );
        let ev = parse_ics(&ics).expand("f", ts(2026, 9, 1, 0, 0), ts(2027, 1, 1, 0, 0), &tz());
        assert_eq!(ev[0].start, ts(2026, 9, 15, 15, 0)); // 14:00 BST = 13:00Z = 15:00 SAST
        assert_eq!(ev[1].start, ts(2026, 12, 15, 16, 0)); // 14:00 GMT = 16:00 SAST
    }

    #[test]
    fn durations_and_garbage() {
        assert_eq!(parse_duration("-PT15M"), Some(-900));
        assert_eq!(parse_duration("P1DT2H"), Some(93_600));
        assert_eq!(parse_duration("P2W"), Some(1_209_600));
        assert_eq!(parse_duration("nonsense"), None);
        assert_eq!(parse_ics("not a calendar at all").event_count(), 0);
    }
}
