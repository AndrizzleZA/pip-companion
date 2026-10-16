//! How the creature feels. Deliberately simple and forgiving: the pet can get a little thirsty,
//! but it never sulks, never dies and never guilt-trips.

use crate::schedule::is_night;
use crate::settings::Settings;
use crate::state::State;
use crate::time::{in_window, local, to_ts, TimeOfDay};
use chrono::{Datelike, TimeZone};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mood {
    /// Default, relaxed.
    Content,
    /// Recently petted / on track.
    Happy,
    /// Daily water goal reached.
    Proud,
    /// Behind on water — shows a little drop and a wobbly mouth.
    Thirsty,
    /// Quiet hours — naps.
    Sleepy,
}

/// Glasses the user would "ideally" have had by now, pro-rata through today's active window.
pub fn water_expected<Tz: TimeZone>(s: &Settings, now: i64, since: Option<i64>, tz: &Tz) -> Option<f32> {
    let w = &s.water;
    let l = local(tz, now);
    if !w.enabled || !w.days.contains(l.date().weekday()) || w.end <= w.start {
        return None;
    }
    let ws = to_ts(tz, w.start.on(l.date()));
    let we = to_ts(tz, w.end.on(l.date()));
    let from = since.map_or(ws, |t| t.max(ws)).min(we);
    let frac = ((now - from) as f32 / (we - ws) as f32).clamp(0.0, 1.0);
    Some(frac * w.daily_goal as f32)
}

pub fn mood<Tz: TimeZone>(s: &Settings, st: &State, now: i64, tz: &Tz) -> Mood {
    if is_night(s, now, tz) {
        return Mood::Sleepy;
    }
    let today = local(tz, now).date();
    let count = if st.water.day == Some(today) { st.water.count } else { 0 };
    let recently_loved = now - st.last_interaction < 10 * 60;
    if s.water.enabled && count >= s.water.daily_goal {
        return Mood::Proud;
    }
    let in_active = in_window(TimeOfDay::of(local(tz, now)), s.water.start, s.water.end);
    if let (true, Some(expected)) = (in_active, water_expected(s, now, st.water.seen_since.filter(|&t| local(tz, t).date() == today), tz)) {
        if (count as f32) + 1.5 < expected && !recently_loved {
            return Mood::Thirsty;
        }
        if count as f32 >= expected || recently_loved {
            return Mood::Happy;
        }
        return Mood::Content;
    }
    if recently_loved || st.affection > 70 {
        Mood::Happy
    } else {
        Mood::Content
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, NaiveDate};

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(7200).unwrap()
    }
    fn at(h: u32, m: u32) -> i64 {
        to_ts(&tz(), NaiveDate::from_ymd_opt(2026, 9, 21).unwrap().and_hms_opt(h, m, 0).unwrap())
    }

    #[test]
    fn moods() {
        let s = Settings::default();
        let mut st = State::new(0);
        st.water.day = Some(NaiveDate::from_ymd_opt(2026, 9, 21).unwrap());
        assert_eq!(mood(&s, &st, at(23, 0), &tz()), Mood::Sleepy);
        assert_eq!(mood(&s, &st, at(14, 0), &tz()), Mood::Thirsty); // 0 of ~4
        st.water.count = 4;
        assert_eq!(mood(&s, &st, at(14, 0), &tz()), Mood::Happy);
        st.water.count = 8;
        assert_eq!(mood(&s, &st, at(14, 0), &tz()), Mood::Proud);
        // Met at 13:30 today: not expected to have caught up on the morning.
        st.water.count = 0;
        st.water.seen_since = Some(at(13, 30));
        assert_ne!(mood(&s, &st, at(14, 0), &tz()), Mood::Thirsty);
    }
}
