//! Noticing when a background job has gone quiet.
//!
//! Pip's work happens out of sight: email checks, the calendar, the phone. When one of them stops
//! there is nothing to see — no error, no empty screen — because "nothing needed you" and "this
//! stopped working" look exactly alike. Three of those stoppages were found by accident, one of
//! them only because a water count on a phone disagreed with a PC.
//!
//! So each job says when it last finished, and this decides whether that was too long ago. The
//! answer is deliberately pure: the judgement is the part worth testing, and it has no business
//! knowing about timers or windows.

/// Has a job gone quiet?
///
/// `last_ok` is when it last finished, or 0 if it never has — in which case it is judged from
/// `since`, when Pip started, because a job that has never run once is the worst case, not an
/// excused one. `every` is the job's own interval.
///
/// The allowance is generous on purpose. A watchdog that cries wolf over one slow round gets
/// ignored, and the stoppages worth catching are measured in hours, not minutes.
pub fn gone_quiet(last_ok: i64, since: i64, every: i64, now: i64) -> bool {
    let baseline = if last_ok > 0 { last_ok } else { since };
    now.saturating_sub(baseline) > allowance(every)
}

/// Three rounds, and never less than ten minutes, so a job that runs every minute isn't declared
/// dead over a slow network.
fn allowance(every: i64) -> i64 {
    every.saturating_mul(3).max(10 * 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_791_000_000;
    const EVERY: i64 = 6 * 60; // the email default

    #[test]
    fn a_job_that_just_ran_is_fine() {
        assert!(!gone_quiet(NOW - 60, NOW - 3600, EVERY, NOW));
    }

    #[test]
    fn one_slow_round_is_not_a_stoppage() {
        // Twice its interval: late, but the kind of late a busy morning explains.
        assert!(!gone_quiet(NOW - 2 * EVERY, NOW - 3600, EVERY, NOW));
    }

    #[test]
    fn hours_of_silence_is() {
        assert!(gone_quiet(NOW - 3 * 3600, NOW - 4 * 3600, EVERY, NOW));
    }

    #[test]
    fn a_job_that_has_never_run_is_judged_from_startup() {
        // Exactly the phone sync bug: nothing ever armed it, so it had no last_ok to be late from.
        assert!(gone_quiet(0, NOW - 3 * 3600, EVERY, NOW), "never ran, and Pip has been up for hours");
        assert!(!gone_quiet(0, NOW - 60, EVERY, NOW), "but a fresh start is given time to get going");
    }

    #[test]
    fn a_fast_job_still_gets_ten_minutes() {
        let minutely = 60;
        assert!(!gone_quiet(NOW - 9 * 60, NOW - 3600, minutely, NOW), "nine minutes is not dead");
        assert!(gone_quiet(NOW - 11 * 60, NOW - 3600, minutely, NOW));
    }

    #[test]
    fn a_slow_job_gets_three_rounds_not_ten_minutes() {
        let hourly = 3600;
        assert!(!gone_quiet(NOW - 2 * hourly, NOW - 9999, hourly, NOW));
        assert!(gone_quiet(NOW - 4 * hourly, NOW - 9999, hourly, NOW));
    }

    #[test]
    fn a_clock_that_jumps_backwards_doesnt_raise_an_alarm() {
        // Daylight saving, or the user correcting the time: last_ok can end up in the future.
        assert!(!gone_quiet(NOW + 3600, NOW - 3600, EVERY, NOW));
    }
}
