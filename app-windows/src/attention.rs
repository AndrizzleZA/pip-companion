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
    /// What it does when the first gesture finishes, or `None` to stop there.
    ///
    /// This is the follow-through, and it is why the reminders stopped looking like one-shot
    /// twitches: anticipation, reaction, and a settling beat afterwards. It costs no timer -- the
    /// frame loop is already running for the first animation and simply finds a second one waiting
    /// when it ends (`App::on_frame`). One deep, never a queue: two is a sequence, three is a
    /// performance nobody asked for, and a chain that can refill itself is a creature that never
    /// stops moving.
    pub follow: Option<AnimKind>,
    pub sound: Option<Sfx>,
    /// Minutes until the next attempt, or `None` when this was the last one.
    pub again_in: Option<u32>,
}

/// How loudly one attempt is allowed to be made.
///
/// Derived from the item's own priority and the style the person chose, in that order: an
/// important email is loud because it is important, and `Insistent` is loud because it was asked
/// for. Nothing else in here reads `Priority` or `Attention` again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Urgency {
    /// Barely anything: a glance. Low-priority items and the quietest style.
    Quiet,
    /// A gesture that means the thing it is about.
    Plain,
    /// Anticipation, then a performance. Important mail lives here.
    Loud,
}

impl Urgency {
    fn of(style: Attention, priority: Priority) -> Urgency {
        match (priority, style) {
            (Priority::Low, _) => Urgency::Quiet,
            (_, Attention::Subtle) => Urgency::Plain,
            (Priority::High, _) | (_, Attention::Insistent) => Urgency::Loud,
            _ => Urgency::Plain,
        }
    }
}

/// What one creature does about one kind of reminder: a gesture, and what it does afterwards.
///
/// Two rules hold this table together. **The first gesture says what the reminder is** — water is
/// always the thirsty mime, whichever creature is doing it, because a reminder you have to decode
/// is not a reminder. **The follow-on says who is doing it** — that is where a bunny differs from
/// a pebble, and it is the part that varies.
///
/// The one exception is important mail, where the creature matters enough to change the main
/// gesture too: four of them come at you and three of them stay put and point. A pebble sprinting
/// across the desk would be funny once and wrong every time after (§64 settled that it does not
/// hop, and this is the same creature).
/// `vital` is the item's own importance, which is not the same thing as the urgency: somebody who
/// chose `Insistent` gets bigger gestures about everything, and the performance still belongs to
/// the mail that is genuinely important. Without that distinction, turning the style up made every
/// email a rush, and then the rush meant "you have mail" instead of "you have *this* mail".
fn manner(who: Character, key: &ItemKey, urge: Urgency, vital: bool, roll: u64) -> (AnimKind, Option<AnimKind>) {
    use AnimKind as A;
    // A glance is the whole of a quiet ask, for everything. There is nothing to vary: the point of
    // it is that it is not a performance.
    if urge == Urgency::Quiet {
        return (A::Peer, None);
    }
    let loud = urge == Urgency::Loud;
    // Which of a creature's two or three ways of reacting this particular reminder gets. It is the
    // only randomness in here, and it never changes *what* the gesture means — see the doc above.
    let pick = (roll % 3) as u8;
    let comes_at_you = matches!(who, Character::Sprout | Character::Bunny | Character::Tuft | Character::Moth);

    match key {
        // -- Water. The mime first, always; the creature's own opinion about it second. ---------
        ItemKey::Water => {
            let after = match who {
                Character::Sprout => (!loud).then_some(A::Nod).or(Some(A::Point)),
                // Impatient about it: a hop, or a nudge at your hand.
                Character::Bunny => Some(if pick == 0 { A::Hop } else { A::Point }),
                Character::Kit => Some(if pick == 0 { A::Curious } else { A::Settle }),
                // Shakes itself off, the way something small and fluffy does about everything.
                Character::Tuft => Some(if pick == 0 { A::Shake } else { A::Sneeze }),
                // Sits down next to the problem. Pebble's answer to most things.
                Character::Pebble => Some(A::Settle),
                Character::Bloom => Some(if pick == 0 { A::Stretch } else { A::Wiggle }),
                // Cannot hold still even while being thirsty.
                Character::Moth => Some(if pick == 0 { A::Fidget } else { A::Hop }),
            };
            (A::Thirsty, if loud { after } else { after.filter(|_| pick != 2) })
        }

        // -- Important email. The one the brief was mostly about. -------------------------------
        ItemKey::Mail => {
            if !loud {
                // Not urgent: it has noticed something and it tells you where to look.
                let first = match who {
                    Character::Kit | Character::Pebble => A::Curious,
                    _ => A::Alert,
                };
                return (first, Some(A::Point));
            }
            // Urgent, and the one place where the creature changes the whole shape of the
            // reaction rather than just the end of it. Each of the seven startles in its own
            // way — fluff shakes itself, a moth is already fidgeting, a bunny is airborne — and
            // then four of them come at you and three stay put.
            let first = match (who, pick) {
                (Character::Tuft, _) => A::Shake,
                (Character::Bunny, 0) => A::Hop,
                (Character::Moth, 0) => A::Fidget,
                (Character::Kit, 0) => A::Curious,
                (Character::Bloom, 0) => A::Wiggle,
                _ => A::Alert,
            };
            let second = if comes_at_you && vital {
                A::Rush
            } else {
                // The three that stay put knock at the screen instead, which is the loudest thing
                // a creature can do without leaving the spot it was standing on — and it is not
                // what any ordinary reminder does, which is the point.
                A::Tap
            };
            (first, Some(second))
        }

        // -- A date. Pleased, not instructive. --------------------------------------------------
        ItemKey::Date(_) => {
            let first = match who {
                Character::Pebble => A::Wiggle,
                Character::Kit => {
                    if pick == 0 {
                        A::Curious
                    } else {
                        A::Celebrate
                    }
                }
                Character::Bunny | Character::Moth => {
                    if pick == 0 {
                        A::Hop
                    } else {
                        A::Celebrate
                    }
                }
                _ => A::Celebrate,
            };
            (first, if loud { Some(A::Dance) } else { (pick == 0).then_some(A::Wiggle) })
        }

        // -- The morning summary. It has just got up; it behaves like it. -----------------------
        ItemKey::Brief => {
            let first = match who {
                Character::Pebble | Character::Bloom => A::Yawn,
                _ => A::Stretch,
            };
            let after = match who {
                Character::Pebble => Some(A::Settle),
                Character::Bloom => Some(A::Stretch),
                Character::Tuft => Some(A::Shake),
                Character::Moth => Some(A::LookAround),
                _ => (pick == 0).then_some(A::Nod),
            };
            (first, if loud { Some(A::Point) } else { after })
        }

        // -- Calendar events and the person's own reminders. ------------------------------------
        //
        // These are the ones with no mime available: there is no body language for "stand-up at
        // ten". So this is where the restlessness lives — a creature that has been waiting for you
        // to get on with something — and where the variation matters most, because these are the
        // reminders people have several of a day.
        ItemKey::Event(_) | ItemKey::Custom(_) => {
            let first = if loud {
                A::Alert
            } else {
                match (who, pick) {
                    (Character::Pebble, 0) => A::Fidget,
                    (Character::Pebble, _) => A::Peer,
                    (Character::Kit, 0) => A::Curious,
                    (Character::Bunny | Character::Moth, 0) => A::Hop,
                    (Character::Tuft, 0) => A::Shake,
                    (Character::Bloom, 0) => A::Stretch,
                    (_, 1) => A::Fidget,
                    _ => A::Wave,
                }
            };
            // Never [`AnimKind::Rush`], however urgent a stand-up is. Coming at you across the
            // desk is important mail's and nothing else's — the moment a second reminder borrows
            // it, the first one stops meaning anything, and "my email performance" was the whole
            // of what was asked for.
            //
            // The point is compulsory after a wave or a glance, because neither of those says
            // anything about *which* reminder this is on its own, and optional after the rest,
            // which is where the quiet periods in a day come from.
            let after = if loud || matches!(first, A::Wave | A::Peer) {
                Some(A::Point)
            } else {
                (pick == 0).then_some(A::Point)
            };
            (first, after)
        }
    }
}

/// The second and third attempts, which must not repeat the first.
///
/// Different rather than louder: the same wave played twice reads as a stuck program, where a
/// different gesture reads as the same creature trying again. A retry never gets the performance
/// either — [`AnimKind::Rush`] belongs to the first time a thing is said, and a creature that
/// sprints at you twice about one email is the nagging §7 refuses.
fn retry_gesture(attempt: u8, key: &ItemKey, who: Character) -> (AnimKind, Option<AnimKind>) {
    use AnimKind as A;
    match (attempt, key) {
        (1, ItemKey::Water) => (A::Tap, Some(A::Thirsty)),
        (1, ItemKey::Mail) => (A::Point, Some(A::Tap)),
        (1, _) => (A::Wave, Some(A::Point)),
        (_, ItemKey::Water) => (if who == Character::Pebble { A::Fidget } else { A::Dance }, None),
        _ => (A::Tap, None),
    }
}

/// One question put to [`act`]: what is due, how much it matters, which creature is being asked
/// to say so, how many times it has already tried, and what the person's switches say.
///
/// It is a struct rather than eight arguments because it reached eight arguments, and because
/// every one of them is read somewhere in the answer: there is nothing here to drop.
#[derive(Clone, Copy, Debug)]
pub struct Ask<'a> {
    pub style: Attention,
    pub who: Character,
    pub key: &'a ItemKey,
    pub priority: Priority,
    /// 0 the first time an item is presented, counting up.
    pub attempt: u8,
    /// The person's sound switch.
    pub sound_on: bool,
    /// Quiet hours, during which nothing makes a noise at all even when an important item is
    /// allowed through (§13).
    pub quiet: bool,
    /// The only source of variation in here. The caller supplies it, so this stays a function of
    /// its arguments and nothing else.
    pub roll: u64,
}

/// Decides one attempt.
pub fn act(ask: Ask) -> Act {
    let Ask { style, who, key, priority, attempt, sound_on, quiet, roll } = ask;
    let silent = !sound_on || quiet || priority == Priority::Low;
    if style == Attention::Still {
        // The bubble is the notification. Nothing else happens, including on the first attempt.
        return Act { anim: None, follow: None, sound: None, again_in: None };
    }
    if attempt > style.retries() {
        return Act { anim: None, follow: None, sound: None, again_in: None };
    }

    let urge = Urgency::of(style, priority);
    let (anim, follow) = if attempt == 0 {
        let (a, f) = manner(who, key, urge, priority == Priority::High, roll);
        // Subtle means one small gesture, which is what it has always meant: the gesture that
        // suits the reminder, and then nothing. The style is the ceiling on the performance, not
        // on which creature is performing.
        (a, if style == Attention::Subtle { None } else { f })
    } else {
        retry_gesture(attempt, key, who)
    };

    let sound = if silent {
        None
    } else if attempt == 0 {
        Some(match (key, urge) {
            // The surprised squeak, under the double-take. The only reminder that gets it.
            (ItemKey::Mail, Urgency::Loud) => Sfx::Chirp,
            (ItemKey::Water, _) => Sfx::Nudge,
            (ItemKey::Date(_), _) => Sfx::Trill,
            (ItemKey::Brief, _) => Sfx::Sigh,
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

    Act { anim: Some(anim), follow, sound, again_in }
}

#[cfg(test)]
mod asking {
    use super::*;


    /// Positional, because a test that spells out eight field names nine times over stops being
    /// readable as a list of cases.
    #[allow(clippy::too_many_arguments)]
    fn put(
        style: Attention,
        who: Character,
        key: &ItemKey,
        priority: Priority,
        attempt: u8,
        sound_on: bool,
        quiet: bool,
        roll: u64,
    ) -> Act {
        act(Ask { style, who, key, priority, attempt, sound_on, quiet, roll })
    }
    fn water() -> ItemKey {
        ItemKey::Water
    }

    /// The old signature, for the promises that have nothing to do with which creature it is.
    /// Sprout and a roll of zero: the companion everybody starts with, behaving predictably.
    fn ask(style: Attention, key: &ItemKey, priority: Priority, attempt: u8, sound_on: bool, quiet: bool) -> Act {
        put(style, Character::Sprout, key, priority, attempt, sound_on, quiet, 0)
    }

    /// Every kind of reminder there is, with the two string-carrying ones filled in.
    fn all_keys() -> Vec<ItemKey> {
        vec![
            ItemKey::Water,
            ItemKey::Mail,
            ItemKey::Brief,
            ItemKey::Date("d".into()),
            ItemKey::Event("e".into()),
            ItemKey::Custom("c".into()),
        ]
    }

    /// The promise this module exists to keep: it always runs out of attempts.
    #[test]
    fn every_style_stops_asking() {
        for style in Attention::ALL {
            let mut attempt = 0u8;
            // Follow the ladder the way the app does, and assert it terminates well inside any
            // patience anybody has.
            for _ in 0..10 {
                let a = ask(style, &water(), Priority::High, attempt, true, false);
                match a.again_in {
                    Some(_) => attempt += 1,
                    None => break,
                }
            }
            assert!(attempt <= 2, "{style:?} wanted {attempt} further attempts");
            let after = ask(style, &water(), Priority::High, attempt + 1, true, false);
            assert_eq!(after.anim, None, "{style:?} still animates past its own ceiling");
            assert_eq!(after.follow, None, "{style:?} queues a second beat past its own ceiling");
            assert_eq!(after.again_in, None, "{style:?} asks for yet another attempt");
        }
    }

    /// Still means still: the bubble, and nothing whatsoever around it.
    #[test]
    fn still_does_nothing_at_all() {
        for key in [water(), ItemKey::Mail, ItemKey::Brief] {
            let a = ask(Attention::Still, &key, Priority::High, 0, true, false);
            assert_eq!(a, Act { anim: None, follow: None, sound: None, again_in: None });
        }
    }

    /// Quiet hours are silent even for the important items that may still appear.
    #[test]
    fn nothing_makes_a_noise_during_quiet_hours() {
        for style in Attention::ALL {
            for attempt in 0..3 {
                let a = ask(style, &water(), Priority::High, attempt, true, true);
                assert_eq!(a.sound, None, "{style:?} attempt {attempt} made a noise at night");
            }
        }
    }

    /// And the sound switch is a switch.
    #[test]
    fn sound_off_means_no_sound_but_still_a_gesture() {
        let a = ask(Attention::Playful, &water(), Priority::Normal, 0, false, false);
        assert_eq!(a.sound, None);
        assert!(a.anim.is_some(), "turning sound off should not make the companion stop moving");
    }

    /// Low priority has always been bubble-only as far as noise goes; that does not change here.
    #[test]
    fn low_priority_stays_quiet() {
        let a = ask(Attention::Insistent, &ItemKey::Mail, Priority::Low, 0, true, false);
        assert_eq!(a.sound, None);
    }

    /// A second attempt that looks exactly like the first reads as a stuck program.
    #[test]
    fn a_second_try_is_a_different_gesture() {
        for key in [water(), ItemKey::Mail, ItemKey::Custom("x".into()), ItemKey::Date("d".into())] {
            let first = ask(Attention::Insistent, &key, Priority::Normal, 0, true, false);
            let second = ask(Attention::Insistent, &key, Priority::Normal, 1, true, false);
            let third = ask(Attention::Insistent, &key, Priority::Normal, 2, true, false);
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
            let a = ask(Attention::Playful, key, Priority::Normal, 0, true, false).anim.expect("a gesture");
            assert!(!seen.contains(&a), "{key:?} does the same thing as something else");
            seen.push(a);
        }
    }

    /// Important things come back sooner, and nothing comes back immediately.
    #[test]
    fn a_retry_waits_a_sensible_while() {
        let high = ask(Attention::Insistent, &water(), Priority::High, 0, true, false).again_in.expect("a retry");
        let normal = ask(Attention::Insistent, &water(), Priority::Normal, 0, true, false).again_in.expect("a retry");
        assert!(high >= 3, "{high} minutes is not a pause, it is a flicker");
        assert!(high < normal, "an important item should come back sooner than an ordinary one");
    }

    // -- What §68 added: the reminder vocabulary, per creature --------------------------------

    /// Every reminder, every creature, every style that moves at all: something happens, and the
    /// something is never nothing. A creature that silently ignores one kind of reminder is a bug
    /// nobody would see until the day that reminder mattered.
    #[test]
    fn nobody_is_ever_left_with_nothing_to_do() {
        for who in Character::ALL {
            for key in all_keys() {
                for prio in [Priority::Low, Priority::Normal, Priority::High] {
                    for style in [Attention::Subtle, Attention::Playful, Attention::Insistent] {
                        for roll in 0..6u64 {
                            let a = put(style, who, &key, prio, 0, true, false, roll);
                            assert!(
                                a.anim.is_some(),
                                "{who:?} does nothing about {key:?} at {prio:?}/{style:?} (roll {roll})"
                            );
                        }
                    }
                }
            }
        }
    }

    /// A sequence of one gesture twice is a stutter, not a sequence.
    #[test]
    fn no_sequence_repeats_its_own_first_beat() {
        for who in Character::ALL {
            for key in all_keys() {
                for prio in [Priority::Normal, Priority::High] {
                    for roll in 0..9u64 {
                        let a = put(Attention::Insistent, who, &key, prio, 0, true, false, roll);
                        if let (Some(first), Some(then)) = (a.anim, a.follow) {
                            assert_ne!(first, then, "{who:?} plays {first:?} twice for {key:?}");
                        }
                    }
                }
            }
        }
    }

    /// The headline promise of the whole change: coming at you across the desk means an important
    /// email and nothing else. The moment a water nudge borrows it, it stops meaning anything.
    #[test]
    fn only_important_mail_ever_rushes() {
        for who in Character::ALL {
            for key in all_keys() {
                for prio in [Priority::Low, Priority::Normal, Priority::High] {
                    for style in Attention::ALL {
                        for roll in 0..9u64 {
                            let a = put(style, who, &key, prio, 0, true, false, roll);
                            let rushes = a.anim == Some(AnimKind::Rush) || a.follow == Some(AnimKind::Rush);
                            if rushes {
                                assert_eq!(key, ItemKey::Mail, "{who:?} rushes about {key:?}");
                                assert_eq!(prio, Priority::High, "{who:?} rushes about ordinary mail");
                            }
                        }
                    }
                }
            }
        }
        // And the four creatures that do run really do run, or the promise is empty.
        let runners = Character::ALL
            .into_iter()
            .filter(|&who| {
                (0..9).any(|roll| {
                    put(Attention::Playful, who, &ItemKey::Mail, Priority::High, 0, true, false, roll).follow
                        == Some(AnimKind::Rush)
                })
            })
            .count();
        assert_eq!(runners, 4, "{runners} creatures come at you about important mail, not four");
    }

    /// An important email must not be mistakable for a stand-up at ten. Same urgency, same
    /// creature, different thing happening.
    #[test]
    fn important_mail_never_looks_like_an_ordinary_reminder() {
        for who in Character::ALL {
            for roll in 0..9u64 {
                let mail = put(Attention::Playful, who, &ItemKey::Mail, Priority::High, 0, true, false, roll);
                for key in all_keys() {
                    if key == ItemKey::Mail {
                        continue;
                    }
                    let other = put(Attention::Playful, who, &key, Priority::High, 0, true, false, roll);
                    assert_ne!(
                        (mail.anim, mail.follow),
                        (other.anim, other.follow),
                        "{who:?} treats {key:?} exactly like an important email (roll {roll})"
                    );
                }
            }
        }
    }

    /// Seven creatures, seven ways of being startled. Not seven copies of one startle with
    /// different noises over the top, which is what the brief asked us not to do.
    #[test]
    fn no_two_creatures_react_to_mail_alike() {
        let mut seen = Vec::new();
        for who in Character::ALL {
            let a = put(Attention::Insistent, who, &ItemKey::Mail, Priority::High, 0, true, false, 0);
            seen.push((a.anim, a.follow));
        }
        let mut distinct = seen.clone();
        distinct.sort_by_key(|p| format!("{p:?}"));
        distinct.dedup();
        assert_eq!(distinct.len(), Character::ALL.len(), "only {} of seven reactions differ: {seen:?}", distinct.len());
    }

    /// Low priority is a glance and nothing else, whatever the style and whatever the creature:
    /// the one place in here where everything behaves identically, because "barely anything" has
    /// only one shape.
    #[test]
    fn a_quiet_ask_is_only_ever_a_glance() {
        for who in Character::ALL {
            for key in all_keys() {
                for style in [Attention::Subtle, Attention::Playful, Attention::Insistent] {
                    let a = put(style, who, &key, Priority::Low, 0, true, false, 7);
                    assert_eq!(a.anim, Some(AnimKind::Peer), "{who:?} performs about a low-priority {key:?}");
                    assert_eq!(a.follow, None, "{who:?} queues a second beat for a glance");
                    assert_eq!(a.sound, None);
                }
            }
        }
    }

    /// §64 settled that a pebble does not hop, and a reminder is no reason to make it. The same
    /// creature in both places, or the temperament table was decoration.
    #[test]
    fn a_pebble_stays_a_pebble_about_everything() {
        for key in all_keys() {
            for prio in [Priority::Low, Priority::Normal, Priority::High] {
                for style in Attention::ALL {
                    for roll in 0..9u64 {
                        let a = put(style, Character::Pebble, &key, prio, 0, true, false, roll);
                        for g in [a.anim, a.follow].into_iter().flatten() {
                            assert!(
                                !matches!(g, AnimKind::Hop | AnimKind::Pounce | AnimKind::Rush | AnimKind::Stumble),
                                "a pebble does {g:?} about {key:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The same reminder twice in a day should not be the same performance twice. Water keeps its
    /// mime — that is the part that means "water" — so the variation lives in what comes after.
    #[test]
    fn the_same_reminder_is_not_always_identical() {
        for who in Character::ALL {
            for key in [ItemKey::Water, ItemKey::Custom("c".into()), ItemKey::Event("e".into())] {
                let mut seen = Vec::new();
                for roll in 0..12u64 {
                    let a = put(Attention::Playful, who, &key, Priority::Normal, 0, true, false, roll);
                    if !seen.contains(&(a.anim, a.follow)) {
                        seen.push((a.anim, a.follow));
                    }
                }
                assert!(seen.len() >= 2, "{who:?} always does exactly the same thing about {key:?}");
            }
        }
    }

    /// Water is the one reminder with a mime of its own, and it always leads with it: a reminder
    /// you have to decode is not a reminder.
    #[test]
    fn asking_for_water_always_starts_by_asking_for_water() {
        for who in Character::ALL {
            for prio in [Priority::Normal, Priority::High] {
                for roll in 0..9u64 {
                    let a = put(Attention::Playful, who, &ItemKey::Water, prio, 0, true, false, roll);
                    assert_eq!(a.anim, Some(AnimKind::Thirsty), "{who:?} opens a water nudge with something else");
                }
            }
        }
    }

    /// Subtle was asked for by somebody who wants less, and it has to keep meaning less: one
    /// gesture, no sequence, no performance.
    #[test]
    fn subtle_is_one_gesture_and_then_nothing() {
        for who in Character::ALL {
            for key in all_keys() {
                for roll in 0..6u64 {
                    let a = put(Attention::Subtle, who, &key, Priority::High, 0, true, false, roll);
                    assert!(a.anim.is_some());
                    assert_eq!(a.follow, None, "{who:?} performs a sequence about {key:?} on the subtle setting");
                    assert_ne!(a.anim, Some(AnimKind::Rush), "{who:?} rushes on the subtle setting");
                }
            }
        }
    }

    /// Nothing in here may outlast the patience of the person it is aimed at. Two beats, and the
    /// longest pair there is still has to be over inside five seconds.
    #[test]
    fn no_sequence_goes_on_too_long() {
        for who in Character::ALL {
            for key in all_keys() {
                for prio in [Priority::Normal, Priority::High] {
                    for roll in 0..9u64 {
                        let a = put(Attention::Insistent, who, &key, prio, 0, true, false, roll);
                        let ms: u64 = [a.anim, a.follow].into_iter().flatten().map(AnimKind::duration_ms).sum();
                        assert!(ms <= 5_000, "{who:?} spends {ms} ms on {key:?}, which is a cut scene");
                    }
                }
            }
        }
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
