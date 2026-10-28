//! Which small thing the companion does next, and how often it does nothing at all.
//!
//! The old answer was `options[rand % options.len()]`: every idle equally likely, every time, so a
//! "rare" one was only rare if the list was long, and the same two could land back to back. This
//! is the same idea with three things added — a weight, a cooldown, and a real chance of doing
//! nothing — which is most of what separates a creature from a slideshow.
//!
//! Deliberately *not* a scheduler. It owns no timers and runs nothing; it answers one question
//! when the existing idle timer asks, and the cadence of that timer is untouched. Picking from a
//! longer list costs nothing while nothing is playing, and that is the whole performance story
//! (§56).

use crate::render::creature::AnimKind;
use pip_core::settings::Character;

/// The quietest setting a behaviour is allowed at: 0 everywhere, 1 Normal and up, 2 Lively only.
type Level = u8;

struct Idle {
    kind: AnimKind,
    /// Relative chance against everything else eligible at that moment.
    weight: u16,
    /// Smallest gap before this may play again.
    cooldown_s: u64,
    floor: Level,
    /// Worth more in the hour before bed, and skipped when the pet should be settling down.
    sleepy: bool,
    /// Needs some energy behind it. Skipped when the companion is thirsty or flat, on the same
    /// reasoning §28 gives for never guilt-tripping: the mood shows in what it does, not in a
    /// message about what it is not doing.
    perky: bool,
}

const IDLES: &[Idle] = &[
    Idle { kind: AnimKind::LookAround, weight: 20, cooldown_s: 45, floor: 0, sleepy: false, perky: false },
    Idle { kind: AnimKind::Settle, weight: 18, cooldown_s: 50, floor: 0, sleepy: false, perky: false },
    Idle { kind: AnimKind::Yawn, weight: 6, cooldown_s: 240, floor: 0, sleepy: true, perky: false },
    Idle { kind: AnimKind::Stretch, weight: 14, cooldown_s: 90, floor: 1, sleepy: false, perky: false },
    Idle { kind: AnimKind::Wiggle, weight: 12, cooldown_s: 90, floor: 1, sleepy: false, perky: false },
    Idle { kind: AnimKind::Sniff, weight: 12, cooldown_s: 120, floor: 1, sleepy: false, perky: false },
    Idle { kind: AnimKind::Scratch, weight: 10, cooldown_s: 150, floor: 1, sleepy: false, perky: false },
    Idle { kind: AnimKind::Hop, weight: 10, cooldown_s: 120, floor: 2, sleepy: false, perky: true },
    Idle { kind: AnimKind::Curious, weight: 12, cooldown_s: 150, floor: 1, sleepy: false, perky: false },
    Idle { kind: AnimKind::Shake, weight: 9, cooldown_s: 200, floor: 1, sleepy: false, perky: false },
    // The ones that leave the spot they were put in. Rarer than the rest on purpose: a companion
    // that is forever off somewhere is one you have to look for.
    Idle { kind: AnimKind::Wander, weight: 5, cooldown_s: 420, floor: 1, sleepy: false, perky: false },
    Idle { kind: AnimKind::Pounce, weight: 5, cooldown_s: 360, floor: 2, sleepy: false, perky: true },
    // Genuinely rare: a weight of one or two against ~a hundred, and a quarter of an hour or more
    // before they are candidates again. Things you notice a few times a day and mention to someone.
    Idle { kind: AnimKind::Sneeze, weight: 1, cooldown_s: 900, floor: 1, sleepy: false, perky: false },
    Idle { kind: AnimKind::Stumble, weight: 2, cooldown_s: 1200, floor: 2, sleepy: false, perky: true },
    Idle { kind: AnimKind::Found, weight: 1, cooldown_s: 2700, floor: 1, sleepy: false, perky: false },
];

/// How much of the time the pet simply sits there, as a weight against all the rest.
///
/// It has to be a real share rather than a leftover: a companion that does something every single
/// time its timer fires is a companion you end up watching instead of working, and §7 says the pet
/// never grabs attention it was not given.
const DO_NOTHING_WEIGHT: u16 = 55;

/// What a sleeping companion does, and how rarely.
///
/// **There is no continuous breathing, and that was the one real decision here.** A gentle breath
/// running all night means the animation clock running all night: about 65 seconds of CPU over nine
/// hours, against a target of one second per hour (§8). It would have been the single most
/// expensive thing in the program, to animate something nobody is looking at — Pip only idles at
/// all while somebody is actually at the machine, because going away stops the timer outright.
///
/// So sleep moves the way the waking pet does: occasionally, in short pieces, with long gaps. A
/// breath that arrives every few minutes is also *more* noticeable than one that never stops.
const SLEEP: &[Idle] = &[
    Idle { kind: AnimKind::SleepBreath, weight: 24, cooldown_s: 90, floor: 0, sleepy: false, perky: false },
    Idle { kind: AnimKind::SleepTwitch, weight: 14, cooldown_s: 180, floor: 0, sleepy: false, perky: false },
    Idle { kind: AnimKind::SleepRoll, weight: 10, cooldown_s: 300, floor: 0, sleepy: false, perky: false },
    Idle { kind: AnimKind::SleepPeek, weight: 6, cooldown_s: 600, floor: 0, sleepy: false, perky: false },
    // The bad dream. Once or twice a night at most, which is the whole joke.
    Idle { kind: AnimKind::SleepStartle, weight: 2, cooldown_s: 2400, floor: 0, sleepy: false, perky: false },
];

/// Sleeping is mostly lying still, more so than being awake is sitting still.
const SLEEP_NOTHING_WEIGHT: u16 = 40;

/// How much more or less likely *this* creature is to do that, in tenths: 10 is the common
/// temperament, 30 is three times as likely, 0 is never.
///
/// **Personality here is odds, not new drawings**, and that is the whole design. §24 says the
/// desktop and the phone must never end up with two versions of the creature, and seven creatures
/// with seven private animation sets is exactly how that rule dies — seven times the drawing, seven
/// times the floor tests, and six chances to forget one of them. So every creature keeps the same
/// vocabulary and differs in how often it reaches for each word, which is most of what tells two
/// animals apart when you watch them anyway: a rabbit and a tortoise both sit, hop and look round.
///
/// It costs one table lookup inside a choice that only happens when an idle is already due, which
/// is to say nothing measurable (§63 measured the idle loop at half a second an hour and this adds
/// a multiply to it).
///
/// The floors are deliberately *not* touched: a taste of 30 cannot drag a Lively-only behaviour
/// down to a Calm companion, because somebody who asked for calm asked for calm and no amount of
/// character gets to overrule it.
fn taste(who: Character, kind: AnimKind) -> u16 {
    use AnimKind as A;
    match who {
        // The baseline every other temperament is measured against, and it does not move. Sprout
        // is the creature this project's drawing was tuned on and the one every screenshot shows;
        // a Sprout that changed the day characters got personalities would be a regression wearing
        // a feature's clothes.
        Character::Sprout => 10,

        // All spring. The one creature where hopping is the first idea rather than the last.
        Character::Bunny => match kind {
            A::Hop => 30,
            A::Pounce => 20,
            A::Wiggle => 16,
            A::Wander => 15,
            A::Sniff => 13,
            A::Settle => 7,
            A::SleepTwitch => 15,
            A::SleepPeek => 14,
            _ => 10,
        },

        // A toadstool. Things rooted to the spot watch, and they are the better watchers for it.
        Character::Kit => match kind {
            A::Settle => 18,
            A::Curious => 15,
            A::LookAround => 14,
            A::Sniff => 13,
            A::Stretch => 8,
            A::Wander => 5,
            A::Hop => 4,
            A::Pounce => 3,
            A::SleepPeek => 13,
            _ => 10,
        },

        // Shaggy, so it is forever dealing with its own coat.
        Character::Tuft => match kind {
            A::Shake => 30,
            A::Scratch => 25,
            A::Sneeze => 18,
            A::Stretch => 14,
            A::Sniff => 12,
            A::Hop => 8,
            A::SleepRoll => 15,
            A::SleepTwitch => 13,
            _ => 10,
        },

        // Broad, low and bald: the outline is the whole character, so the outline mostly stays
        // where it is. The three it never does are not rarity, they are a flat zero — a creature
        // shaped like a stone that pounces reads as a bug, not as a surprise.
        Character::Pebble => match kind {
            A::Hop | A::Pounce | A::Stumble => 0,
            A::Settle => 22,
            A::LookAround => 15,
            A::Yawn => 15,
            A::Wiggle => 4,
            A::Wander => 3,
            A::Shake => 5,
            A::Scratch => 6,
            A::Stretch => 6,
            A::Sniff => 8,
            A::SleepBreath => 13,
            A::SleepTwitch => 4,
            A::SleepRoll => 3,
            A::SleepStartle => 3,
            A::SleepPeek => 5,
            _ => 10,
        },

        // A ring of petals. It opens, it closes, and the pollen is its own problem.
        Character::Bloom => match kind {
            A::Stretch => 25,
            A::Sneeze => 20,
            A::Wiggle => 18,
            A::Settle => 13,
            A::Shake => 12,
            A::Hop => 6,
            A::Scratch => 5,
            A::Wander => 4,
            A::Pounce => 4,
            A::SleepBreath => 14,
            _ => 10,
        },

        // The smallest, and the only one with antennae to lead it off somewhere. Flighty in both
        // senses: it goes, and it startles.
        Character::Moth => match kind {
            A::Wander => 25,
            A::Hop => 22,
            A::Curious => 20,
            A::Sniff => 18,
            A::Stumble => 15,
            A::Found => 15,
            A::Pounce => 12,
            A::Scratch => 12,
            A::Stretch => 8,
            A::Settle => 5,
            A::SleepTwitch => 20,
            A::SleepStartle => 18,
            A::SleepRoll => 15,
            A::SleepPeek => 15,
            _ => 10,
        },
    }
}

pub struct Idles {
    /// When each behaviour last played, parallel to [`IDLES`] and [`SLEEP`].
    awake_last: [u64; IDLES.len()],
    sleep_last: [u64; SLEEP.len()],
    /// Whatever went last, which never goes twice running however the dice fall.
    awake_prev: Option<usize>,
    sleep_prev: Option<usize>,
    started: bool,
}

impl Default for Idles {
    fn default() -> Idles {
        Idles {
            awake_last: [0; IDLES.len()],
            sleep_last: [0; SLEEP.len()],
            awake_prev: None,
            sleep_prev: None,
            started: false,
        }
    }
}

/// Weighted pick from one table, or `None` for "do nothing".
///
/// `roll` is a single random number and every decision is made from it, which is what makes this
/// testable at all: the same roll always gives the same answer.
fn choose(
    table: &[Idle],
    last: &mut [u64],
    prev: &mut Option<usize>,
    nothing: u16,
    now_ms: u64,
    who: Character,
    level: Level,
    evening: bool,
    flat: bool,
    roll: u64,
) -> Option<AnimKind> {
    let mut total = u32::from(nothing);
    let mut eligible = [0u32; 20];
    for (i, idle) in table.iter().enumerate() {
        if idle.floor > level || *prev == Some(i) || (idle.perky && flat) {
            continue;
        }
        if now_ms.saturating_sub(last[i]) < idle.cooldown_s * 1000 {
            continue;
        }
        // A yawn means something at bedtime and nothing at ten in the morning.
        let w = if idle.sleepy {
            if evening {
                idle.weight * 5
            } else {
                1
            }
        } else {
            idle.weight
        };
        // And then what this particular creature thinks of it. A taste of zero is a real "never",
        // not a long cooldown, so it drops out here rather than being picked rarely.
        let w = u32::from(w) * u32::from(taste(who, idle.kind)) / 10;
        if w == 0 {
            continue;
        }
        eligible[i] = w;
        total += w;
    }

    let mut target = (roll % u64::from(total)) as u32;
    for (i, w) in eligible.iter().take(table.len()).enumerate() {
        let w = *w;
        if w == 0 {
            continue;
        }
        if target < w {
            last[i] = now_ms;
            *prev = Some(i);
            return Some(table[i].kind);
        }
        target -= w;
    }
    // Fell through to the do-nothing share. The previous pick is cleared so that sitting still
    // does not block whatever went before it from coming round again.
    *prev = None;
    None
}

impl Idles {
    /// A cooldown measured from zero would make everything eligible at the first tick after a
    /// launch, including the rare ones. The clock starts when the pet does.
    fn start(&mut self, now_ms: u64) {
        if !self.started {
            self.started = true;
            self.awake_last = [now_ms; IDLES.len()];
            self.sleep_last = [now_ms; SLEEP.len()];
        }
    }

    /// Picks the next idle for a pet that is awake, or `None` for "sit there".
    pub fn pick(
        &mut self,
        now_ms: u64,
        who: Character,
        level: Level,
        evening: bool,
        flat: bool,
        roll: u64,
    ) -> Option<AnimKind> {
        self.start(now_ms);
        let Idles { awake_last, awake_prev, .. } = self;
        choose(IDLES, awake_last, awake_prev, DO_NOTHING_WEIGHT, now_ms, who, level, evening, flat, roll)
    }

    /// And for one curled up asleep in its burrow. Liveliness does not apply: a Calm companion
    /// still breathes, and a Lively one does not have livelier dreams. The creature does apply —
    /// how an animal sleeps is as much itself as how it moves.
    pub fn pick_asleep(&mut self, now_ms: u64, who: Character, roll: u64) -> Option<AnimKind> {
        self.start(now_ms);
        let Idles { sleep_last, sleep_prev, .. } = self;
        choose(SLEEP, sleep_last, sleep_prev, SLEEP_NOTHING_WEIGHT, now_ms, who, 0, false, false, roll)
    }
}

#[cfg(test)]
mod picking {
    use super::*;

    fn counts(who: Character, level: Level, evening: bool, rolls: u64) -> (usize, Vec<(AnimKind, usize)>) {
        let mut idles = Idles::default();
        let mut nothing = 0;
        let mut seen: Vec<(AnimKind, usize)> = Vec::new();
        // An hour of pretend time per roll, so cooldowns never mask the weights being tested.
        for r in 0..rolls {
            let now = 10_000_000 + r * 3_600_000;
            match idles.pick(now, who, level, evening, false, r.wrapping_mul(6_364_136_223_846_793_005).rotate_left(17)) {
                None => nothing += 1,
                Some(k) => match seen.iter_mut().find(|(s, _)| *s == k) {
                    Some((_, n)) => *n += 1,
                    None => seen.push((k, 1)),
                },
            }
        }
        (nothing, seen)
    }

    /// Calm means calm: nothing above the quietest tier is ever a candidate.
    #[test]
    fn calm_only_ever_gets_the_quiet_ones() {
        let (_, seen) = counts(Character::Sprout, 0, false, 3_000);
        for (kind, _) in seen {
            assert!(
                matches!(kind, AnimKind::LookAround | AnimKind::Settle | AnimKind::Yawn),
                "{kind:?} is not something Calm should ever do"
            );
        }
    }

    /// Sitting still has to be a common outcome, not a rounding error.
    #[test]
    fn the_pet_often_does_nothing_at_all() {
        let (nothing, _) = counts(Character::Sprout, 2, false, 3_000);
        let share = nothing as f32 / 3_000.0;
        assert!((0.2..0.45).contains(&share), "doing nothing happened {:.0}% of the time", share * 100.0);
    }

    /// A rare animation has to actually be rare, which is the half of "rare" that gets forgotten.
    #[test]
    fn the_rare_one_stays_rare() {
        let (_, seen) = counts(Character::Sprout, 2, false, 3_000);
        let sneezes = seen.iter().find(|(k, _)| *k == AnimKind::Sneeze).map_or(0, |(_, n)| *n);
        let common = seen.iter().find(|(k, _)| *k == AnimKind::LookAround).map_or(0, |(_, n)| *n);
        assert!(sneezes > 0, "it should still happen sometimes");
        assert!(sneezes * 8 < common, "{sneezes} sneezes against {common} looks around is not rare");
    }

    /// Back-to-back repeats are what made the old pool feel mechanical.
    #[test]
    fn nothing_plays_twice_in_a_row() {
        let mut idles = Idles::default();
        let mut prev = None;
        for r in 0..2_000u64 {
            let now = 10_000_000 + r * 3_600_000;
            let got = idles.pick(now, Character::Sprout, 2, false, false, r.wrapping_mul(2_862_933_555_777_941_757).rotate_left(29));
            if let (Some(a), Some(b)) = (got, prev) {
                assert_ne!(a, b, "{a:?} played twice running");
            }
            prev = got;
        }
    }

    /// And the cooldown is real: ask twice in quick succession and the second cannot repeat it.
    #[test]
    fn a_cooldown_keeps_something_from_coming_straight_back() {
        let mut idles = Idles::default();
        // The first call only starts the clock — everything is on cooldown from the moment the
        // pet launches, so that a rare animation cannot land in the first minute.
        assert_eq!(idles.pick(10_000_000, Character::Sprout, 2, false, false, 7), None, "nothing is eligible on the first tick");

        let t = 10_000_000 + 20 * 60_000;
        let mut first = None;
        for r in 0..40u64 {
            if let Some(k) = idles.pick(t + r, Character::Sprout, 2, false, false, r * 7_919 + 3) {
                first = Some(k);
                break;
            }
        }
        let first = first.expect("something to play once the cooldowns have passed");

        // Now ask repeatedly, far enough apart that `prev` is not what is stopping it, but well
        // inside every cooldown in the table.
        let mut seen_again = false;
        for r in 0..200u64 {
            if idles.pick(t + 1_000 + r, Character::Sprout, 2, false, false, r * 97 + 13) == Some(first) {
                seen_again = true;
            }
        }
        assert!(!seen_again, "{first:?} came back inside its own cooldown");
    }

    /// A yawn belongs to the evening, and should hardly ever turn up at other times.
    #[test]
    fn yawning_belongs_to_the_end_of_the_day() {
        let day = counts(Character::Sprout, 1, false, 3_000).1.iter().find(|(k, _)| *k == AnimKind::Yawn).map_or(0, |(_, n)| *n);
        let eve = counts(Character::Sprout, 1, true, 3_000).1.iter().find(|(k, _)| *k == AnimKind::Yawn).map_or(0, |(_, n)| *n);
        assert!(eve > day * 4, "{eve} yawns in the evening against {day} in the day is not a difference");
    }
}

#[cfg(test)]
mod sleeping {
    use super::*;

    fn nights(who: Character, rolls: u64) -> (usize, Vec<(AnimKind, usize)>) {
        let mut idles = Idles::default();
        let mut nothing = 0;
        let mut seen: Vec<(AnimKind, usize)> = Vec::new();
        for r in 0..rolls {
            // An hour of pretend time per roll, so cooldowns do not mask the weights.
            let now = 50_000_000 + r * 3_600_000;
            match idles.pick_asleep(now, who, r.wrapping_mul(6_364_136_223_846_793_005).rotate_left(23)) {
                None => nothing += 1,
                Some(k) => match seen.iter_mut().find(|(s, _)| *s == k) {
                    Some((_, n)) => *n += 1,
                    None => seen.push((k, 1)),
                },
            }
        }
        (nothing, seen)
    }

    /// Everything a sleeping companion does has to be something a sleeping companion would do.
    #[test]
    fn a_sleeping_pet_only_does_sleeping_things() {
        let (_, seen) = nights(Character::Sprout, 2_000);
        assert!(!seen.is_empty(), "it should do something now and then");
        for (kind, _) in seen {
            assert!(kind.is_sleep_talk(), "{kind:?} is not something to do while asleep");
        }
    }

    /// Mostly it just lies there, which is the point of being asleep.
    #[test]
    fn sleep_is_mostly_lying_still() {
        let (nothing, _) = nights(Character::Sprout, 2_000);
        let share = nothing as f32 / 2_000.0;
        assert!((0.25..0.55).contains(&share), "it lay still {:.0}% of the time", share * 100.0);
    }

    /// A nightmare you saw every night would stop being funny on the second one.
    #[test]
    fn the_nightmare_is_rare() {
        let (_, seen) = nights(Character::Sprout, 2_000);
        let bad = seen.iter().find(|(k, _)| *k == AnimKind::SleepStartle).map_or(0, |(_, n)| *n);
        let breath = seen.iter().find(|(k, _)| *k == AnimKind::SleepBreath).map_or(0, |(_, n)| *n);
        assert!(bad > 0, "it should still happen");
        assert!(bad * 6 < breath, "{bad} nightmares against {breath} breaths is not rare");
    }

    /// The two tables are separate: being awake all day must not use up the night's cooldowns.
    #[test]
    fn the_waking_day_does_not_eat_the_nights_cooldowns() {
        let mut idles = Idles::default();
        for r in 0..500u64 {
            idles.pick(60_000_000 + r * 60_000, Character::Sprout, 2, false, false, r * 7_919 + 1);
        }
        let mut slept = 0;
        for r in 0..60u64 {
            if idles.pick_asleep(60_000_000 + 500 * 60_000 + r * 120_000, Character::Sprout, r * 104_729 + 7).is_some() {
                slept += 1;
            }
        }
        assert!(slept > 5, "only {slept} sleep behaviours in an hour of night");
    }
}

#[cfg(test)]
mod mood {
    use super::*;

    /// A thirsty companion has less bounce in it. It never says so — it just does the quieter
    /// things, which is how a mood reads on anything alive.
    #[test]
    fn a_flat_mood_takes_the_bounce_out() {
        let mut idles = Idles::default();
        let mut seen = Vec::new();
        for r in 0..3_000u64 {
            let now = 90_000_000 + r * 3_600_000;
            if let Some(k) = idles.pick(now, Character::Sprout, 2, false, true, r.wrapping_mul(6_364_136_223_846_793_005).rotate_left(11)) {
                if !seen.contains(&k) {
                    seen.push(k);
                }
            }
        }
        assert!(!seen.is_empty(), "it should still do things, just quieter ones");
        for kind in seen {
            assert!(
                !matches!(kind, AnimKind::Hop | AnimKind::Pounce | AnimKind::Stumble),
                "{kind:?} is too bouncy for a flat mood"
            );
        }
    }

    /// And the bouncy ones do come back once it is feeling better.
    #[test]
    fn a_good_mood_gets_them_back() {
        let mut idles = Idles::default();
        let mut bouncy = 0;
        for r in 0..3_000u64 {
            let now = 90_000_000 + r * 3_600_000;
            if let Some(k) = idles.pick(now, Character::Sprout, 2, false, false, r.wrapping_mul(6_364_136_223_846_793_005).rotate_left(11)) {
                if matches!(k, AnimKind::Hop | AnimKind::Pounce | AnimKind::Stumble) {
                    bouncy += 1;
                }
            }
        }
        assert!(bouncy > 50, "only {bouncy} bouncy behaviours in 3000 picks");
    }
}

#[cfg(test)]
mod temperament {
    use super::*;

    /// Counts what one creature actually did over a long stretch of pretend time.
    fn tally(who: Character, level: Level, rolls: u64) -> (usize, Vec<(AnimKind, usize)>) {
        let mut idles = Idles::default();
        let (mut nothing, mut seen) = (0, Vec::<(AnimKind, usize)>::new());
        for r in 0..rolls {
            // An hour apart, so cooldowns never stand in for the weights being tested.
            let now = 20_000_000 + r * 3_600_000;
            match idles.pick(now, who, level, false, false, r.wrapping_mul(6_364_136_223_846_793_005).rotate_left(13)) {
                None => nothing += 1,
                Some(k) => match seen.iter_mut().find(|(s, _)| *s == k) {
                    Some((_, n)) => *n += 1,
                    None => seen.push((k, 1)),
                },
            }
        }
        (nothing, seen)
    }

    fn asleep(who: Character, rolls: u64) -> (usize, Vec<(AnimKind, usize)>) {
        let mut idles = Idles::default();
        let (mut nothing, mut seen) = (0, Vec::<(AnimKind, usize)>::new());
        for r in 0..rolls {
            let now = 70_000_000 + r * 3_600_000;
            match idles.pick_asleep(now, who, r.wrapping_mul(6_364_136_223_846_793_005).rotate_left(19)) {
                None => nothing += 1,
                Some(k) => match seen.iter_mut().find(|(s, _)| *s == k) {
                    Some((_, n)) => *n += 1,
                    None => seen.push((k, 1)),
                },
            }
        }
        (nothing, seen)
    }

    fn how_often(seen: &[(AnimKind, usize)], kind: AnimKind) -> usize {
        seen.iter().find(|(k, _)| *k == kind).map_or(0, |(_, n)| *n)
    }

    /// The original is the baseline and must stay exactly where it was, in every table.
    #[test]
    fn sprout_is_the_creature_it_always_was() {
        for idle in IDLES.iter().chain(SLEEP.iter()) {
            assert_eq!(taste(Character::Sprout, idle.kind), 10, "Sprout's {:?} moved", idle.kind);
        }
    }

    /// A creature shaped like a stone that pounces reads as a bug, not as a surprise. Never is
    /// never, not "rarely".
    #[test]
    fn a_pebble_keeps_its_feet_on_the_ground() {
        let (_, seen) = tally(Character::Pebble, 2, 4_000);
        assert!(!seen.is_empty(), "it should still do plenty");
        for (kind, n) in seen {
            assert!(
                !matches!(kind, AnimKind::Hop | AnimKind::Pounce | AnimKind::Stumble),
                "a Pebble did {kind:?} {n} times"
            );
        }
    }

    /// Personality must not read as "broken". Every creature keeps a varied day and keeps the same
    /// habit of sitting still that §57 settled on.
    #[test]
    fn every_creature_still_has_plenty_to_do() {
        for who in Character::ALL {
            let (nothing, seen) = tally(who, 2, 3_000);
            assert!(seen.len() >= 8, "{} only ever did {} different things", who.label(), seen.len());
            let share = nothing as f32 / 3_000.0;
            assert!(
                (0.15..0.5).contains(&share),
                "{} sat still {:.0}% of the time",
                who.label(),
                share * 100.0
            );
        }
    }

    /// The whole point: watch two of them for a while and they are not the same animal.
    #[test]
    fn no_two_creatures_behave_alike() {
        let mut profiles: Vec<(Character, Vec<(AnimKind, usize)>)> = Vec::new();
        for who in Character::ALL {
            profiles.push((who, tally(who, 2, 3_000).1));
        }
        for (i, (a, pa)) in profiles.iter().enumerate() {
            for (b, pb) in profiles.iter().skip(i + 1) {
                let same = IDLES.iter().all(|idle| how_often(pa, idle.kind) == how_often(pb, idle.kind));
                assert!(!same, "{} and {} behave identically", a.label(), b.label());
            }
        }
    }

    /// Character changes the odds; it never overrules the setting. Somebody who asked for Calm
    /// asked for calm, whichever creature they picked.
    #[test]
    fn calm_stays_calm_whichever_creature_it_is() {
        for who in Character::ALL {
            let (_, seen) = tally(who, 0, 3_000);
            for (kind, _) in seen {
                assert!(
                    matches!(kind, AnimKind::LookAround | AnimKind::Settle | AnimKind::Yawn),
                    "a Calm {} did {kind:?}",
                    who.label()
                );
            }
        }
    }

    /// And no taste table may leave a creature with nothing to do at the quietest setting, which
    /// is the one way a zero could turn into a companion that just sits there forever.
    #[test]
    fn nothing_is_ever_tasted_into_silence() {
        for who in Character::ALL {
            let (_, seen) = tally(who, 0, 2_000);
            assert!(!seen.is_empty(), "a Calm {} never did anything at all", who.label());
        }
    }

    /// How an animal sleeps is as much itself as how it moves.
    #[test]
    fn they_sleep_as_differently_as_they_wake() {
        let (stone_still, stone) = asleep(Character::Pebble, 3_000);
        let (flighty_still, flighty) = asleep(Character::Moth, 3_000);
        assert!(
            stone_still > flighty_still,
            "a Pebble lay still {stone_still} times against a Moth's {flighty_still}"
        );
        let stone_twitch = how_often(&stone, AnimKind::SleepTwitch);
        let moth_twitch = how_often(&flighty, AnimKind::SleepTwitch);
        assert!(moth_twitch > stone_twitch * 2, "{moth_twitch} moth twitches against {stone_twitch}");
        // Both still breathe. A sleeping companion that never moved at all would look switched off.
        assert!(how_often(&stone, AnimKind::SleepBreath) > 0, "a Pebble should still breathe");
    }

    /// The creature it shows as is the creature it behaves as. A settings file naming one that has
    /// not arrived shows Sprout (§31), and it must act like Sprout too rather than borrowing a
    /// temperament nobody can see.
    #[test]
    fn it_behaves_as_the_creature_it_is_drawn_as() {
        let hidden = Character::Moth;
        assert_eq!(hidden.shown(0), Character::Sprout, "a Moth has not arrived on day zero");
        let (_, as_sprout) = tally(hidden.shown(0), 2, 2_000);
        let (_, as_sprout_again) = tally(Character::Sprout, 2, 2_000);
        for idle in IDLES {
            assert_eq!(
                how_often(&as_sprout, idle.kind),
                how_often(&as_sprout_again, idle.kind),
                "{:?} differs between a shown Sprout and a real one",
                idle.kind
            );
        }
    }
}
