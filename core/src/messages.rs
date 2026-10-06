//! The creature's voice. Gentle, short, never shouty. `seq` rotates variants deterministically.

fn pick<'a>(options: &'a [&'a str], seq: u32) -> &'a str {
    options[seq as usize % options.len()]
}

fn fill(t: &str, name: &str) -> String {
    t.replace("{name}", name)
}

pub fn water_nudge(name: &str, seq: u32, count: u32, goal: u32) -> (String, String) {
    const LINES: &[&str] = &[
        "{name} thinks you might need some water.",
        "Tiny sip break? {name} is having one too.",
        "It's been a little while since your last glass.",
        "A glass of water would be lovely right about now.",
        "{name} brought you a gentle reminder: water!",
        "Hydration check. Your future self says thanks.",
    ];
    let body = fill(pick(LINES, seq), name);
    let title = if count >= goal {
        "Bonus sip?".to_string()
    } else {
        format!("Water  ·  {count} of {goal} today")
    };
    (title, body)
}

pub fn water_late(name: &str) -> String {
    fill("Welcome back! {name} saved you a reminder: a glass of water?", name)
}

pub fn water_logged(count: u32, goal: u32, seq: u32) -> String {
    if count == goal {
        return "Goal reached! That's the whole day's water. So proud of you.".into();
    }
    if count > goal {
        return format!("{count} glasses today. Extra hydrated!");
    }
    const LINES: &[&str] = &["Yay! {c} of {g} today.", "Glug glug. {c} of {g}!", "Nice! {c} of {g} done."];
    pick(LINES, seq).replace("{c}", &count.to_string()).replace("{g}", &goal.to_string())
}

pub fn reminder_intro(name: &str, late: bool, at: &str) -> String {
    if late {
        format!("Earlier at {at}. {name} kept it for you.")
    } else {
        format!("{name} is reminding you.")
    }
}

pub fn done_cheer(seq: u32) -> &'static str {
    pick(&["Done! Nicely handled.", "Ticked off. Good job!", "One less thing to remember."], seq)
}

pub fn greeting(name: &str, hour: u32) -> String {
    let part = match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    };
    format!("{part}! {name} is here.")
}

pub fn sleepy_poke(name: &str, seq: u32) -> String {
    fill(pick(&["{name} is napping… zzz", "Shh, {name} is dreaming of water.", "Mmh? Five more minutes…"], seq), name)
}

pub fn first_run(name: &str) -> (String, String) {
    (
        format!("Hi, I'm {name}!"),
        "I'll live here quietly and remind you about water and things you'd like to remember. Drag me anywhere; right-click for options.".into(),
    )
}

/// (title, body) for an important date. `days_until` is 0 on the day itself.
pub fn date_reminder(kind: crate::dates::DateKind, describe: &str, days_until: i64, weekday: &str, name: &str) -> (String, String) {
    use crate::dates::DateKind::*;
    if days_until <= 0 {
        let body = match kind {
            Birthday => "It's today! A message or a call would make their day.".to_string(),
            Anniversary => "It's today. Something small and sweet, maybe?".to_string(),
            Deadline => "It's due today.".to_string(),
            Other => "It's today.".to_string(),
        };
        (describe.to_string(), body)
    } else {
        let when = if days_until == 1 { "tomorrow".to_string() } else { format!("on {weekday} (in {days_until} days)") };
        let verb = if kind == Deadline { "is due" } else { "is coming up" };
        (describe.to_string(), format!("It {verb} {when}. {name} thought you'd like a heads-up."))
    }
}

/// Body for a calendar event alert.
pub fn event_body(all_day: bool, start_hm: &str, minutes_until: i64) -> String {
    if all_day {
        "Today, all day.".into()
    } else if minutes_until > 1 {
        format!("Starts at {start_hm} (in {minutes_until} minutes).")
    } else if minutes_until >= -1 {
        format!("Starting now ({start_hm}).")
    } else {
        format!("Started at {start_hm}.")
    }
}

pub fn brief_title(name: &str, hour: u32) -> String {
    let part = match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    };
    format!("{part}! Here's today, from {name}")
}

/// (title, body lines) for the grouped important-email bubble.
pub fn mail_card(msgs: &[crate::email::EmailMsg]) -> (String, Vec<String>) {
    let mut newest: Vec<&crate::email::EmailMsg> = msgs.iter().collect();
    newest.sort_by_key(|m| std::cmp::Reverse(m.received));
    match newest.as_slice() {
        [] => (String::new(), Vec::new()),
        [m] => (format!("Email from {}", m.sender()), vec![format!("“{}”", m.subject)]),
        many => {
            let mut lines: Vec<String> = many.iter().take(3).map(|m| format!("{} — {}", m.sender(), m.subject)).collect();
            if many.len() > 3 {
                lines.push(format!("…and {} more", many.len() - 3));
            }
            (format!("{} important emails", many.len()), lines)
        }
    }
}

/// How the companion is getting on, for the settings page. Counts up and never mentions a day
/// that didn't go well.
pub fn growth_label(experience: u32) -> String {
    let days = match experience {
        0 => "no good days yet".to_string(),
        1 => "1 good day".to_string(),
        n => format!("{n} good days"),
    };
    let stage = match experience {
        e if e >= crate::state::GROWN => "Fully grown",
        e if e >= crate::state::GROWING => "Growing up",
        _ => "Just arrived",
    };
    format!("{stage} \u{00B7} {days}")
}

/// Said once, when a companion reaches a new stage.
pub fn grew_up(stage: u8, name: &str) -> String {
    match stage {
        1 => format!("{name} is growing up!"),
        _ => format!("{name} is all grown up!"),
    }
}

#[cfg(test)]
mod growth_labels {
    use super::*;

    #[test]
    fn it_counts_up_and_never_mentions_a_missed_day() {
        assert_eq!(growth_label(0), "Just arrived \u{00B7} no good days yet");
        assert_eq!(growth_label(1), "Just arrived \u{00B7} 1 good day");
        assert_eq!(growth_label(14), "Just arrived \u{00B7} 14 good days");
        assert_eq!(growth_label(15), "Growing up \u{00B7} 15 good days");
        assert_eq!(growth_label(44), "Growing up \u{00B7} 44 good days");
        assert_eq!(growth_label(45), "Fully grown \u{00B7} 45 good days");
        assert_eq!(growth_label(900), "Fully grown \u{00B7} 900 good days");
    }

    #[test]
    fn reaching_a_stage_is_worth_saying() {
        assert_eq!(grew_up(1, "Pip"), "Pip is growing up!");
        assert_eq!(grew_up(2, "Pip"), "Pip is all grown up!");
    }
}
