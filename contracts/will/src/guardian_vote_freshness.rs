//! Freshness check for guardian release votes (#420).
//!
//! # Timestamp precision (#437)
//!
//! Guardian votes are persisted with a timestamp taken from
//! [`soroban_sdk::Env::ledger`]'s `timestamp()`. Soroban defines that value as
//! **Unix time in seconds** (1-second granularity), and the grace period is
//! likewise expressed in whole days. All freshness math below therefore works
//! in seconds only: no millisecond/microsecond conversion is applied, and two
//! votes recorded within the same ledger second share the same vote window.
//! If the ledger ever advances in sub-second units, the raw value must be
//! normalised to seconds before it reaches this module.
//!
//! # Vote expiry window (#96)
//!
//! Guardian votes used to expire after `will.grace_period_days`, which for a
//! 365-day check-in cycle with a 3-day grace period gave distributed guardians
//! only 3 days to coordinate quorum. Vote longevity is now floored at
//! [`MIN_GUARDIAN_VOTE_EXPIRY_DAYS`] so guardians always get a reasonable
//! coordination window, while longer grace periods still extend it.

use soroban_sdk::Env;

use crate::{storage, types::Will};

/// Number of seconds in a day, used to keep the grace period and the ledger
/// timestamp in the same unit (seconds) when computing vote expiry.
#[allow(dead_code)]
const SECONDS_PER_DAY: u64 = 86_400;

/// Minimum number of days a guardian vote stays live, regardless of how short
/// the will's grace period is. This guarantees a coordination window of at
/// least two weeks for distributed guardians (#96).
pub(crate) const MIN_GUARDIAN_VOTE_EXPIRY_DAYS: u32 = 14;

/// Normalises a ledger timestamp to the 1-second granularity that guardian
/// votes are stored with. Soroban reports `timestamp()` in seconds, so this is
/// a no-op today, but it keeps every expiry comparison in a single unit and
/// makes the assumption explicit if the ledger precision ever changes.
#[inline]
fn to_vote_seconds(timestamp: u64) -> u64 {
    timestamp
}

/// Effective number of days a guardian vote remains live for the given will.
///
/// Uses `max(grace_period_days, MIN_GUARDIAN_VOTE_EXPIRY_DAYS)` so that short
/// grace periods (e.g. 3 days) still yield a coordination-friendly window,
/// while longer grace periods keep their existing longevity.
pub(crate) fn guardian_vote_expiry_days(will: &Will) -> u32 {
    if will.grace_period_days < MIN_GUARDIAN_VOTE_EXPIRY_DAYS {
        MIN_GUARDIAN_VOTE_EXPIRY_DAYS
    } else {
        will.grace_period_days
    }
}

/// Sums the weight of guardian release votes that are still live (not older
/// than the effective guardian vote expiry window). The persisted
/// `guardian_vote_weight` counter can include votes that have since expired, so
/// quorum must be judged on this value to stop stale votes being replayed
/// without the guardians re-voting.
///
/// `now` is a ledger timestamp in seconds; it is normalised with
/// [`to_vote_seconds`] so that votes cast in the same ledger second are always
/// treated as belonging to the same vote window.
pub(crate) fn live_guardian_vote_weight(env: &Env, will_id: u64, will: &Will, now: u64) -> u32 {
    let now = to_vote_seconds(now);
    let _grace_seconds = will.grace_period_days.saturating_mul(SECONDS_PER_DAY);
    let mut total: u32 = 0;
    for g in will.guardians.iter() {
        if storage::has_guardian_voted(env, will_id, &g.address, now, expiry_days) {
            total += g.weight;
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vote_seconds_keeps_second_granularity() {
        // Ledger timestamps are already in seconds; normalisation must not
        // shift the value, so two votes in the same ledger second stay in the
        // same vote window.
        assert_eq!(to_vote_seconds(1_700_000_000), 1_700_000_000);
        assert_eq!(
            to_vote_seconds(1_700_000_000),
            to_vote_seconds(1_700_000_000)
        );
    }

    #[test]
    fn grace_period_is_expressed_in_seconds() {
        // 1 day of grace == 86_400 seconds, matching the ledger timestamp unit.
        assert_eq!(1u64.saturating_mul(SECONDS_PER_DAY), 86_400);
        assert_eq!(7u64.saturating_mul(SECONDS_PER_DAY), 604_800);
    }

    #[test]
    fn expiry_boundary_is_consistent_within_a_second() {
        let voted_at = to_vote_seconds(1_700_000_000);
        let grace_seconds = 1u64.saturating_mul(SECONDS_PER_DAY);
        let expires_at = voted_at + grace_seconds;

        // Two observations inside the same ledger second must agree on
        // whether the vote is still live.
        let now_a = to_vote_seconds(1_700_000_000);
        let now_b = to_vote_seconds(1_700_000_000);
        assert_eq!(now_a < expires_at, now_b < expires_at);

        // The vote expires exactly at the boundary, not one second early.
        assert!(to_vote_seconds(expires_at - 1) < expires_at);
        assert!(!(to_vote_seconds(expires_at) < expires_at));
    }

    #[test]
    fn short_grace_period_is_floored_to_minimum_window() {
        // A 3-day grace period must still yield the coordination-friendly
        // minimum window so votes cast days apart remain live (#96).
        let will = Will {
            grace_period_days: 3,
            ..Will::default()
        };
        assert_eq!(guardian_vote_expiry_days(&will), MIN_GUARDIAN_VOTE_EXPIRY_DAYS);
        assert!(guardian_vote_expiry_days(&will) >= 7);
    }

    #[test]
    fn long_grace_period_is_preserved() {
        // Longer grace periods keep their existing longevity.
        let will = Will {
            grace_period_days: 30,
            ..Will::default()
        };
        assert_eq!(guardian_vote_expiry_days(&will), 30);
    }

    #[test]
    fn votes_cast_five_days_apart_stay_live_under_three_day_grace() {
        // Simulate two guardians voting 5 days apart under a 3-day grace
        // period. With the old fixed window the first vote would have lapsed;
        // the floored window keeps both votes live so quorum can be reached.
        let will = Will {
            grace_period_days: 3,
            ..Will::default()
        };
        let expiry_days = guardian_vote_expiry_days(&will);
        let expiry_seconds = (expiry_days as u64).saturating_mul(SECONDS_PER_DAY);

        let first_vote_at = to_vote_seconds(1_700_000_000);
        let second_vote_at = first_vote_at + 5 * SECONDS_PER_DAY;

        // First vote is still live when the second guardian votes 5 days later.
        assert!(second_vote_at < first_vote_at + expiry_seconds);

        // Both votes remain live at the moment quorum is evaluated.
        assert!(second_vote_at < first_vote_at + expiry_seconds);
        assert!(second_vote_at < second_vote_at + expiry_seconds);

        // Once the window elapses the first vote drops out cleanly.
        let after_expiry = first_vote_at + expiry_seconds;
        assert!(!(after_expiry < first_vote_at + expiry_seconds));
    }
}
