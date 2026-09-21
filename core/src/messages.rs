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
