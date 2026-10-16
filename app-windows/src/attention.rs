//! How the companion asks to be noticed, and when it stops asking.
//!
//! One decision in one place: given what is due, how important it is, how many times it has
//! already tried and what the person asked for, this returns a single *act* — one animation, at
//! most one sound, and whether to try again later. Everything that plays a reminder goes through
//! here, so there is no second opinion about what a water nudge looks like.
//!
//! It owns no timers, starts nothing and reads no clock. The caller supplies the attempt number
//! and acts on the answer, which is what makes the whole thing testable without a window.
//!
//! **It stops.** `Attention::retries` is the only place the ceiling is written, and `act` returns
//! `again_in` as `None` once it is reached. Nothing here ever mentions having been ignored: a
//! second attempt is a different gesture, not a complaint (§7, §28).

use crate::render::creature::AnimKind;
use pip_core::schedule::{ItemKey, Priority};
use pip_core::settings::Attention;

/// A short noise with the companion's own timbre. Which notes, not which file — these are
/// synthesised at the moment they play (`platform::Chime`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sfx {
    /// The plain two-note chime: the voice the person chose, unchanged.
    Voice,
    /// Rising, questioning. "Here a moment?"
    Nudge,
    /// Two dry taps, like a knuckle on glass.
    Tap,
    /// A pleased little run upward.
    Trill,
    /// Falling and soft, for the end of the day.
    Sigh,
}

/// What the companion does about one due item, once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Act {
    pub anim: Option<AnimKind>,
    pub sound: Option<Sfx>,
    /// Minutes until the next attempt, or `None` when this was the last one.
    pub again_in: Option<u32>,
}

/// The gesture that suits each kind of reminder.
///
/// Water is a mime of the thing being asked for, mail is a look at the screen the mail is on, a
/// date is pleased rather than instructive. They are reused from the existing library wherever one
/// already fits — a new animation per reminder type would be four more things to keep working for
/// no more meaning than this.
fn gesture(key: &ItemKey, big: bool) -> AnimKind {
    match key {
        ItemKey::Water => AnimKind::Drink,
        ItemKey::Mail => {
            if big {
                AnimKind::Tap
            } else {
                AnimKind::Curious
            }
        }
        ItemKey::Date(_) => {
            if big {
                AnimKind::Dance
            } else {
                AnimKind::Celebrate
            }
        }
        ItemKey::Brief => AnimKind::Stretch,
        ItemKey::Event(_) | ItemKey::Custom(_) => {
            if big {
                AnimKind::Dance
            } else {
                AnimKind::Wave
            }
        }
    }
}

/// The second and third attempts, which must not repeat the first.
///
/// Different rather than louder: the same wave played twice reads as a stuck program, where a
/// different gesture reads as the same creature trying again.
fn retry_gesture(attempt: u8, key: &ItemKey) -> AnimKind {
    match (attempt, key) {
        (1, ItemKey::Water) => AnimKind::Tap,
        (1, _) => AnimKind::Wave,
        (_, ItemKey::Water) => AnimKind::Dance,
        _ => AnimKind::Tap,
    }
}

/// Decides one attempt.
///
/// `attempt` is 0 for the first time an item is presented and counts up. `sound_on` is the
/// person's sound switch; `quiet` is quiet hours, during which nothing makes a noise at all even
/// when an important item is allowed through (§13).
pub fn act(style: Attention, key: &ItemKey, priority: Priority, attempt: u8, sound_on: bool, quiet: bool) -> Act {
    let silent = !sound_on || quiet || priority == Priority::Low;
    if style == Attention::Still {
        // The bubble is the notification. Nothing else happens, including on the first attempt.
        return Act { anim: None, sound: None, again_in: None };
    }
    if attempt > style.retries() {
        return Act { anim: None, sound: None, again_in: None };
    }

    let big = style == Attention::Insistent;
    let anim = if attempt == 0 {
        match style {
            // Subtle keeps the old behaviour: a wave, whatever is due.
            Attention::Subtle => AnimKind::Wave,
            _ => gesture(key, big),
        }
    } else {
        retry_gesture(attempt, key)
    };

    let sound = if silent {
        None
    } else if attempt == 0 {
        Some(match key {
            ItemKey::Water => Sfx::Nudge,
            ItemKey::Date(_) => Sfx::Trill,
            ItemKey::Brief => Sfx::Sigh,
            _ => Sfx::Voice,
        })
    } else {
        // A retry is quieter than the first ask, not louder. The animation carries it.
        Some(Sfx::Tap)
    };

    // Later attempts come sooner for important things and never sooner than a few minutes.
    let again_in = if attempt >= style.retries() {
        None
    } else {
        Some(match priority {
            Priority::High => 4,
            _ => 9,
        })
    };

    Act { anim: Some(anim), sound, again_in }
}

#[cfg(test)]
mod asking {
    use super::*;

    fn water() -> ItemKey {
        ItemKey::Water
    }

    /// The promise this module exists to keep: it always runs out of attempts.
    #[test]
    fn every_style_stops_asking() {
        for style in Attention::ALL {
            let mut attempt = 0u8;
            // Follow the ladder the way the app does, and assert it terminates well inside any
            // patience anybody has.
            for _ in 0..10 {
                let a = act(style, &water(), Priority::High, attempt, true, false);
                match a.again_in {
                    Some(_) => attempt += 1,
                    None => break,
                }
            }
            assert!(attempt <= 2, "{style:?} wanted {attempt} further attempts");
            let after = act(style, &water(), Priority::High, attempt + 1, true, false);
            assert_eq!(after.anim, None, "{style:?} still animates past its own ceiling");
            assert_eq!(after.again_in, None, "{style:?} asks for yet another attempt");
        }
    }

    /// Still means still: the bubble, and nothing whatsoever around it.
    #[test]
    fn still_does_nothing_at_all() {
        for key in [water(), ItemKey::Mail, ItemKey::Brief] {
            let a = act(Attention::Still, &key, Priority::High, 0, true, false);
            assert_eq!(a, Act { anim: None, sound: None, again_in: None });
        }
    }

    /// Quiet hours are silent even for the important items that may still appear.
    #[test]
    fn nothing_makes_a_noise_during_quiet_hours() {
        for style in Attention::ALL {
            for attempt in 0..3 {
                let a = act(style, &water(), Priority::High, attempt, true, true);
                assert_eq!(a.sound, None, "{style:?} attempt {attempt} made a noise at night");
            }
        }
    }

    /// And the sound switch is a switch.
    #[test]
    fn sound_off_means_no_sound_but_still_a_gesture() {
        let a = act(Attention::Playful, &water(), Priority::Normal, 0, false, false);
        assert_eq!(a.sound, None);
        assert!(a.anim.is_some(), "turning sound off should not make the companion stop moving");
    }

    /// Low priority has always been bubble-only as far as noise goes; that does not change here.
    #[test]
    fn low_priority_stays_quiet() {
        let a = act(Attention::Insistent, &ItemKey::Mail, Priority::Low, 0, true, false);
        assert_eq!(a.sound, None);
    }

    /// A second attempt that looks exactly like the first reads as a stuck program.
    #[test]
    fn a_second_try_is_a_different_gesture() {
        for key in [water(), ItemKey::Mail, ItemKey::Custom("x".into()), ItemKey::Date("d".into())] {
            let first = act(Attention::Insistent, &key, Priority::Normal, 0, true, false);
            let second = act(Attention::Insistent, &key, Priority::Normal, 1, true, false);
            let third = act(Attention::Insistent, &key, Priority::Normal, 2, true, false);
            assert_ne!(first.anim, second.anim, "{key:?} repeats itself on the second try");
            assert_ne!(second.anim, third.anim, "{key:?} repeats itself on the third try");
        }
    }

    /// Different reminders should not all look the same, which is the whole complaint that
    /// started this.
    #[test]
    fn different_reminders_get_different_gestures() {
        let keys = [water(), ItemKey::Mail, ItemKey::Date("d".into()), ItemKey::Brief];
        let mut seen = Vec::new();
        for key in &keys {
            let a = act(Attention::Playful, key, Priority::Normal, 0, true, false).anim.expect("a gesture");
            assert!(!seen.contains(&a), "{key:?} does the same thing as something else");
            seen.push(a);
        }
    }

    /// Important things come back sooner, and nothing comes back immediately.
    #[test]
    fn a_retry_waits_a_sensible_while() {
        let high = act(Attention::Insistent, &water(), Priority::High, 0, true, false).again_in.expect("a retry");
        let normal = act(Attention::Insistent, &water(), Priority::Normal, 0, true, false).again_in.expect("a retry");
        assert!(high >= 3, "{high} minutes is not a pause, it is a flicker");
        assert!(high < normal, "an important item should come back sooner than an ordinary one");
    }
}
