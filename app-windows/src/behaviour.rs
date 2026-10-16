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
/// running all night means the 30 fps clock running all night: about 65 seconds of CPU over nine
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
    level: Level,
    evening: bool,
    flat: bool,
    roll: u64,
) -> Option<AnimKind> {
    let mut total = u32::from(nothing);
    let mut eligible = [0u16; 20];
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
        eligible[i] = w;
        total += u32::from(w);
    }

    let mut target = (roll % u64::from(total)) as u32;
    for (i, w) in eligible.iter().take(table.len()).enumerate() {
        let w = u32::from(*w);
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
    pub fn pick(&mut self, now_ms: u64, level: Level, evening: bool, flat: bool, roll: u64) -> Option<AnimKind> {
        self.start(now_ms);
        let Idles { awake_last, awake_prev, .. } = self;
        choose(IDLES, awake_last, awake_prev, DO_NOTHING_WEIGHT, now_ms, level, evening, flat, roll)
    }

    /// And for one curled up asleep in its burrow. Liveliness does not apply: a Calm companion
    /// still breathes, and a Lively one does not have livelier dreams.
    pub fn pick_asleep(&mut self, now_ms: u64, roll: u64) -> Option<AnimKind> {
        self.start(now_ms);
        let Idles { sleep_last, sleep_prev, .. } = self;
        choose(SLEEP, sleep_last, sleep_prev, SLEEP_NOTHING_WEIGHT, now_ms, 0, false, false, roll)
    }
}

#[cfg(test)]
mod picking {
    use super::*;

    fn counts(level: Level, evening: bool, rolls: u64) -> (usize, Vec<(AnimKind, usize)>) {
        let mut idles = Idles::default();
        let mut nothing = 0;
        let mut seen: Vec<(AnimKind, usize)> = Vec::new();
        // An hour of pretend time per roll, so cooldowns never mask the weights being tested.
        for r in 0..rolls {
            let now = 10_000_000 + r * 3_600_000;
            match idles.pick(now, level, evening, false, r.wrapping_mul(6_364_136_223_846_793_005).rotate_left(17)) {
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
        let (_, seen) = counts(0, false, 3_000);
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
        let (nothing, _) = counts(2, false, 3_000);
        let share = nothing as f32 / 3_000.0;
        assert!((0.2..0.45).contains(&share), "doing nothing happened {:.0}% of the time", share * 100.0);
    }

    /// A rare animation has to actually be rare, which is the half of "rare" that gets forgotten.
    #[test]
    fn the_rare_one_stays_rare() {
        let (_, seen) = counts(2, false, 3_000);
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
            let got = idles.pick(now, 2, false, false, r.wrapping_mul(2_862_933_555_777_941_757).rotate_left(29));
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
        assert_eq!(idles.pick(10_000_000, 2, false, false, 7), None, "nothing is eligible on the first tick");

        let t = 10_000_000 + 20 * 60_000;
        let mut first = None;
        for r in 0..40u64 {
            if let Some(k) = idles.pick(t + r, 2, false, false, r * 7_919 + 3) {
                first = Some(k);
                break;
            }
        }
        let first = first.expect("something to play once the cooldowns have passed");

        // Now ask repeatedly, far enough apart that `prev` is not what is stopping it, but well
        // inside every cooldown in the table.
        let mut seen_again = false;
        for r in 0..200u64 {
            if idles.pick(t + 1_000 + r, 2, false, false, r * 97 + 13) == Some(first) {
                seen_again = true;
            }
        }
        assert!(!seen_again, "{first:?} came back inside its own cooldown");
    }

    /// A yawn belongs to the evening, and should hardly ever turn up at other times.
    #[test]
    fn yawning_belongs_to_the_end_of_the_day() {
        let day = counts(1, false, 3_000).1.iter().find(|(k, _)| *k == AnimKind::Yawn).map_or(0, |(_, n)| *n);
        let eve = counts(1, true, 3_000).1.iter().find(|(k, _)| *k == AnimKind::Yawn).map_or(0, |(_, n)| *n);
        assert!(eve > day * 4, "{eve} yawns in the evening against {day} in the day is not a difference");
    }
}

#[cfg(test)]
mod sleeping {
    use super::*;

    fn nights(rolls: u64) -> (usize, Vec<(AnimKind, usize)>) {
        let mut idles = Idles::default();
        let mut nothing = 0;
        let mut seen: Vec<(AnimKind, usize)> = Vec::new();
        for r in 0..rolls {
            // An hour of pretend time per roll, so cooldowns do not mask the weights.
            let now = 50_000_000 + r * 3_600_000;
            match idles.pick_asleep(now, r.wrapping_mul(6_364_136_223_846_793_005).rotate_left(23)) {
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
        let (_, seen) = nights(2_000);
        assert!(!seen.is_empty(), "it should do something now and then");
        for (kind, _) in seen {
            assert!(kind.is_sleep_talk(), "{kind:?} is not something to do while asleep");
        }
    }

    /// Mostly it just lies there, which is the point of being asleep.
    #[test]
    fn sleep_is_mostly_lying_still() {
        let (nothing, _) = nights(2_000);
        let share = nothing as f32 / 2_000.0;
        assert!((0.25..0.55).contains(&share), "it lay still {:.0}% of the time", share * 100.0);
    }

    /// A nightmare you saw every night would stop being funny on the second one.
    #[test]
    fn the_nightmare_is_rare() {
        let (_, seen) = nights(2_000);
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
            idles.pick(60_000_000 + r * 60_000, 2, false, false, r * 7_919 + 1);
        }
        let mut slept = 0;
        for r in 0..60u64 {
            if idles.pick_asleep(60_000_000 + 500 * 60_000 + r * 120_000, r * 104_729 + 7).is_some() {
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
            if let Some(k) = idles.pick(now, 2, false, true, r.wrapping_mul(6_364_136_223_846_793_005).rotate_left(11)) {
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
            if let Some(k) = idles.pick(now, 2, false, false, r.wrapping_mul(6_364_136_223_846_793_005).rotate_left(11)) {
                if matches!(k, AnimKind::Hop | AnimKind::Pounce | AnimKind::Stumble) {
                    bouncy += 1;
                }
            }
        }
        assert!(bouncy > 50, "only {bouncy} bouncy behaviours in 3000 picks");
    }
}
