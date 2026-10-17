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

use crate::platform::Phrase;
use crate::render::creature::AnimKind;
use pip_core::schedule::{ItemKey, Priority};
use pip_core::settings::{Attention, Character};

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
    /// A short surprised squeak.
    Chirp,
    /// Three low notes, swallowed. Water going down.
    Burble,
    /// Up and a long way back down. Waking, or giving in to bedtime.
    Yawn,
    /// Unhappy, and never about the person: this one belongs to tripping over.
    Whine,
    /// Low and content, for a reminder put off rather than refused.
    Purr,
    /// Being touched. Its own sound, used for nothing else — see [`Sfx::level`].
    Touch,
    /// Four notes climbing, for something finished.
    Cheer,
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

impl Sfx {
    /// How loud this one is, against the person's own volume.
    ///
    /// Not everything deserves the same weight. A click happens dozens of times a day and has to
    /// sit under the conversation; finishing something happens once and can afford to be heard.
    /// Getting this wrong in the quiet direction is recoverable — getting it wrong in the loud
    /// direction is how somebody turns the sound off and never turns it back on.
    pub fn level(self) -> f32 {
        match self {
            // Deliberately the quietest thing in the program.
            Sfx::Touch => 0.30,
            Sfx::Purr | Sfx::Burble => 0.55,
            Sfx::Tap | Sfx::Chirp | Sfx::Whine => 0.65,
            Sfx::Nudge | Sfx::Sigh | Sfx::Yawn => 0.8,
            Sfx::Voice | Sfx::Trill => 0.9,
            // The one noise that marks something finished, and the only one at full weight.
            Sfx::Cheer => 1.0,
        }
    }

    /// The five that have to stay apart from each other inside one creature's dialect. Everything
    /// else is small talk and may share a shape with its neighbour; these five are the ones a
    /// person learns, so two of them sounding identical would cost real information.
    #[cfg(test)]
    const LOAD_BEARING: [Sfx; 5] = [Sfx::Voice, Sfx::Nudge, Sfx::Cheer, Sfx::Whine, Sfx::Touch];
}

/// Which *shape* this creature uses to say that, as opposed to which timbre it says it in.
///
/// §61 gave every companion its own voice — seven sets of notes, harmonics, breath and waver, so
/// no two sound alike. What stayed shared was the phrasing: every creature announced a reminder
/// with the same flat two-note chime and asked the same rising question, in its own timbre. A
/// dialect is the other half. Bunny announces things with a pleased little run because it cannot
/// keep still; Kit knocks, because a mushroom is not a bell; Pebble's chime falls slowly and its
/// celebration is the smallest in the cast, because nothing about a stone is in a hurry.
///
/// **It costs nothing.** These are the twelve shapes that already exist (`platform::Phrase`) dealt
/// out differently per creature — no new synthesis, no new buffers, no files, and the answer is a
/// match arm resolved when a sound is already about to play. Switching companion changes the
/// dialect on the very next noise, because nothing is cached anywhere.
///
/// Shapes may repeat inside one dialect: a creature has fewer noises than it has things to say,
/// which is true of animals. What may not repeat is any two of [`Sfx::LOAD_BEARING`], and a test
/// holds that for all seven.
pub fn dialect(who: Character, sfx: Sfx) -> Phrase {
    // The meaning of each sound, which is also Sprout's dialect: it is the creature the voice was
    // designed on and the one in every screenshot, so it does not move (§64 made the same promise
    // about its behaviour).
    let plain = match sfx {
        Sfx::Voice => Phrase::Plain,
        Sfx::Nudge => Phrase::Rising,
        Sfx::Tap => Phrase::Knock,
        Sfx::Trill => Phrase::Trill,
        Sfx::Sigh => Phrase::Falling,
        Sfx::Chirp => Phrase::Chirp,
        Sfx::Burble => Phrase::Burble,
        Sfx::Yawn => Phrase::Yawn,
        Sfx::Whine => Phrase::Whine,
        Sfx::Purr => Phrase::Purr,
        Sfx::Touch => Phrase::Touch,
        Sfx::Cheer => Phrase::Cheer,
    };
    match who {
        Character::Sprout => plain,

        // Cannot deliver a flat note. It announces, squeaks and overdoes the good news.
        Character::Bunny => match sfx {
            Sfx::Voice => Phrase::Trill,
            Sfx::Nudge => Phrase::Chirp,
            Sfx::Trill => Phrase::Cheer,
            Sfx::Purr => Phrase::Burble,
            _ => plain,
        },

        // Dry and wooden. Its chime is two knocks on a table.
        Character::Kit => match sfx {
            Sfx::Voice => Phrase::Knock,
            Sfx::Tap => Phrase::Plain,
            Sfx::Purr => Phrase::Burble,
            _ => plain,
        },

        // Airy, and asks rather than states. Its surprise comes out as a flutter.
        Character::Tuft => match sfx {
            Sfx::Voice => Phrase::Rising,
            Sfx::Nudge => Phrase::Chirp,
            Sfx::Chirp => Phrase::Trill,
            Sfx::Sigh => Phrase::Yawn,
            _ => plain,
        },

        // Low and unhurried. Nothing it says climbs, its surprise is a dull knock rather than a
        // squeak, and its biggest noise is three notes where everyone else gets four — a stone
        // is pleased quietly.
        Character::Pebble => match sfx {
            Sfx::Voice => Phrase::Falling,
            Sfx::Nudge => Phrase::Plain,
            Sfx::Chirp => Phrase::Knock,
            Sfx::Trill => Phrase::Rising,
            Sfx::Sigh => Phrase::Purr,
            Sfx::Cheer => Phrase::Trill,
            _ => plain,
        },

        // Sweet, and the two swap round: its pleased noise is a small squeak and its surprise is
        // a flutter, like something that opens and closes rather than jumps.
        Character::Bloom => match sfx {
            Sfx::Trill => Phrase::Chirp,
            Sfx::Chirp => Phrase::Trill,
            Sfx::Purr => Phrase::Falling,
            Sfx::Sigh => Phrase::Purr,
            _ => plain,
        },

        // The smallest. Its chime is a squeak, and its celebration is three notes because four
        // would be more noise than there is moth.
        Character::Moth => match sfx {
            Sfx::Voice => Phrase::Chirp,
            Sfx::Trill => Phrase::Plain,
            Sfx::Cheer => Phrase::Trill,
            Sfx::Sigh => Phrase::Yawn,
            Sfx::Purr => Phrase::Burble,
            _ => plain,
        },
    }
}

#[cfg(test)]
mod loudness {
    use super::*;

    /// The click is the sound heard most often by a wide margin, so it has to be the smallest.
    #[test]
    fn nothing_is_quieter_than_a_touch() {
        let all = [
            Sfx::Voice, Sfx::Nudge, Sfx::Tap, Sfx::Trill, Sfx::Sigh, Sfx::Chirp,
            Sfx::Burble, Sfx::Yawn, Sfx::Whine, Sfx::Purr, Sfx::Cheer,
        ];
        for s in all {
            assert!(
                s.level() > Sfx::Touch.level(),
                "{s:?} at {} is no louder than the click at {}",
                s.level(),
                Sfx::Touch.level()
            );
        }
    }

    /// And nothing is loud enough to startle: every level leaves headroom.
    #[test]
    fn everything_stays_within_its_bounds() {
        let all = [
            Sfx::Voice, Sfx::Nudge, Sfx::Tap, Sfx::Trill, Sfx::Sigh, Sfx::Chirp,
            Sfx::Burble, Sfx::Yawn, Sfx::Whine, Sfx::Purr, Sfx::Cheer, Sfx::Touch,
        ];
        for s in all {
            assert!((0.2..=1.0).contains(&s.level()), "{s:?} is at {}", s.level());
        }
    }
}

#[cfg(test)]
mod dialects {
    use super::*;

    const ALL: [Sfx; 12] = [
        Sfx::Voice,
        Sfx::Nudge,
        Sfx::Tap,
        Sfx::Trill,
        Sfx::Sigh,
        Sfx::Chirp,
        Sfx::Burble,
        Sfx::Yawn,
        Sfx::Whine,
        Sfx::Purr,
        Sfx::Touch,
        Sfx::Cheer,
    ];

    fn spoken(who: Character) -> Vec<Phrase> {
        ALL.iter().map(|s| dialect(who, *s)).collect()
    }

    /// The creature the voice was designed on does not move, in sound any more than in behaviour.
    #[test]
    fn sprout_still_speaks_the_plain_language() {
        assert_eq!(dialect(Character::Sprout, Sfx::Voice), Phrase::Plain);
        assert_eq!(dialect(Character::Sprout, Sfx::Nudge), Phrase::Rising);
        assert_eq!(dialect(Character::Sprout, Sfx::Cheer), Phrase::Cheer);
        assert_eq!(dialect(Character::Sprout, Sfx::Touch), Phrase::Touch);
    }

    /// The point of the whole thing: you could tell them apart with your eyes shut.
    #[test]
    fn no_two_creatures_speak_the_same_dialect() {
        for (i, a) in Character::ALL.into_iter().enumerate() {
            for b in Character::ALL.into_iter().skip(i + 1) {
                assert_ne!(spoken(a), spoken(b), "{} and {} say everything alike", a.label(), b.label());
            }
        }
    }

    /// Inside one creature, the five sounds a person actually learns have to stay apart. Small
    /// talk may share a shape; "a reminder is due" and "you touched me" may not.
    #[test]
    fn the_sounds_that_carry_meaning_never_collide() {
        for who in Character::ALL {
            for (i, a) in Sfx::LOAD_BEARING.into_iter().enumerate() {
                for b in Sfx::LOAD_BEARING.into_iter().skip(i + 1) {
                    assert_ne!(
                        dialect(who, a),
                        dialect(who, b),
                        "{} says {a:?} and {b:?} the same way",
                        who.label()
                    );
                }
            }
        }
    }

    /// A click is the sound heard most often by a wide margin and has its own shape, used for
    /// nothing else. No dialect may spend it on something else or borrow something louder for it.
    #[test]
    fn being_touched_is_the_quietest_thing_every_creature_says() {
        for who in Character::ALL {
            assert_eq!(dialect(who, Sfx::Touch), Phrase::Touch, "{} mis-sounds a click", who.label());
            for s in ALL.into_iter().filter(|s| *s != Sfx::Touch) {
                assert_ne!(dialect(who, s), Phrase::Touch, "{} spends the click shape on {s:?}", who.label());
            }
        }
    }

    /// Nothing about a stone is in a hurry, so nothing it says is a quick little squeak.
    #[test]
    fn a_pebble_never_squeaks() {
        for s in ALL {
            assert_ne!(dialect(Character::Pebble, s), Phrase::Chirp, "a Pebble chirped at {s:?}");
        }
    }

    /// Every creature still has the full range available to it — a dialect redistributes the
    /// shapes, it does not shrink the vocabulary down to two noises.
    #[test]
    fn no_creature_is_left_with_only_a_handful_of_noises() {
        for who in Character::ALL {
            let mut distinct: Vec<Phrase> = Vec::new();
            for p in spoken(who) {
                if !distinct.contains(&p) {
                    distinct.push(p);
                }
            }
            assert!(distinct.len() >= 9, "{} only makes {} different noises", who.label(), distinct.len());
        }
    }
}
