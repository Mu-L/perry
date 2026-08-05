//! Adaptive tenuring threshold for the copying (scavenging) minor collector.
//!
//! The copying minor ages nursery survivors through the survivor semispaces
//! and promotes them to old-gen after a fixed number of copies
//! (`GC_COPY_PROMOTION_SURVIVALS`, 4). That fixed age is correct for the
//! generational-hypothesis workloads the scavenge was tuned on (tiny live
//! sets, near-empty survivor spaces) and pathological for large live sets:
//! a program whose survivors essentially never die in the survivor space
//! pays `age × influx` bytes of pure re-copying — measured 4.39 GB / 61 M
//! object-copies on a binary-tree benchmark whose entire live set was
//! ~35 MB, because the same ~3.15 MB survivor cohort ping-ponged between
//! the semispaces on every one of 1427 collections.
//!
//! This module is the survivor-occupancy feedback loop (HotSpot's adaptive
//! `TenuringThreshold`, restated for elastic semispaces). Per copying cycle
//! the collector reports the **Eden survivor influx** — bytes moved out of
//! Eden that were live (copied to a survivor space or promoted). At a
//! threshold of S survivals, steady-state survivor occupancy is
//! `(S−1) × influx` (S−1 resident age cohorts), so the next cycle's
//! threshold is the largest S whose projected occupancy fits the desired
//! survivor size:
//!
//! ```text
//! S = min(4, 1 + desired / influx)
//! ```
//!
//! `desired` is 1/16 of the scavenge nursery cap (1 MB at the default
//! 16 MB cap) — the same effective ratio HotSpot's defaults produce
//! (SurvivorRatio=8, TargetSurvivorRatio=50% ⇒ Eden/16).
//!
//! The influx signal is deliberately *threshold-invariant*: live Eden bytes
//! get moved somewhere at any S, so the feedback loop has a fixed point
//! instead of the oscillation that a post-hoc survivor-occupancy signal
//! produces (at S=1 the survivor space is empty, which would read as "no
//! pressure" and snap the threshold straight back up).
//!
//! Asymmetric response: the threshold *drops* to the computed target
//! immediately (every extra cycle at a too-high threshold re-copies the
//! whole resident cohort), but *rises* one step at a time and only after
//! the target has been above the current value for two consecutive cycles
//! (a single quiet cycle — e.g. a malloc-count trigger firing before Eden
//! filled — must not flush the aging pipeline into a copy burst).
//!
//! There is no env knob here (see CLAUDE.md's GC knob kill-policy): the
//! loop is always on, and its neutral state — influx below `desired` —
//! computes S=4, which is bit-for-bit the previous fixed behaviour.

use super::*;

/// Ceiling and power-on value: the previous fixed threshold.
pub(super) const GC_TENURING_SURVIVALS_MAX: u8 = GC_COPY_PROMOTION_SURVIVALS;

/// Consecutive cycles the computed target must exceed the current threshold
/// before it is raised (by one step).
const RAISE_DEBOUNCE_CYCLES: u8 = 2;

thread_local! {
    static TENURING_SURVIVALS: Cell<u8> = const { Cell::new(GC_TENURING_SURVIVALS_MAX) };
    static RAISE_STREAK: Cell<u8> = const { Cell::new(0) };
}

/// The survivals threshold the next copying minor should promote at:
/// `next_age >= tenuring_survivals()` tenures. In `1..=4`; 4 is the
/// original fixed policy, 1 promotes every live nursery object on first
/// copy.
pub(super) fn tenuring_survivals() -> u8 {
    TENURING_SURVIVALS.with(Cell::get)
}

/// Target steady-state survivor occupancy. `desired / 16` of the nursery
/// cap tracks `PERRY_GC_SCAVENGE_NURSERY_MB` so the two dials stay
/// proportional.
pub(super) fn desired_survivor_bytes() -> usize {
    gc_scavenge_nursery_cap_bytes() / 16
}

/// Hard per-cycle valve: once a single cycle has copied this many bytes
/// into the to-survivor space, `move_young` promotes the remainder of the
/// cycle directly (HotSpot's to-space overflow behaviour). This bounds the
/// one-time copy burst of the *first* heavy cycle, before the feedback
/// loop has a signal — e.g. a program materialising a 16 MB live cohort in
/// its first Eden fill would otherwise copy all 16 MB into a survivor
/// space only to promote it a cycle later.
pub(super) fn survivor_overflow_bytes() -> usize {
    desired_survivor_bytes().saturating_mul(4)
}

pub(super) fn compute_target_survivals(eden_live_bytes: usize, desired_bytes: usize) -> u8 {
    if eden_live_bytes == 0 {
        return GC_TENURING_SURVIVALS_MAX;
    }
    let target = 1 + desired_bytes / eden_live_bytes;
    target.min(GC_TENURING_SURVIVALS_MAX as usize) as u8
}

/// Feed one finished copying-minor cycle into the feedback loop.
/// `eden_live_bytes` is the cycle's Eden survivor influx (bytes moved out
/// of Eden, whether copied to a survivor space or promoted).
pub(super) fn retune_after_scavenge(eden_live_bytes: usize) {
    let current = TENURING_SURVIVALS.with(Cell::get);
    let target = compute_target_survivals(eden_live_bytes, desired_survivor_bytes());
    let next = if target < current {
        RAISE_STREAK.with(|s| s.set(0));
        target
    } else if target > current {
        let streak = RAISE_STREAK.with(|s| s.get()).saturating_add(1);
        if streak >= RAISE_DEBOUNCE_CYCLES {
            RAISE_STREAK.with(|s| s.set(0));
            current + 1
        } else {
            RAISE_STREAK.with(|s| s.set(streak));
            current
        }
    } else {
        RAISE_STREAK.with(|s| s.set(0));
        current
    };
    if next != current {
        TENURING_SURVIVALS.with(|s| s.set(next));
        if std::env::var_os("PERRY_GC_DIAG").is_some() {
            eprintln!(
                "[gc-tenuring] survivals {} -> {} (eden_live_bytes={} desired={})",
                current,
                next,
                eden_live_bytes,
                desired_survivor_bytes()
            );
        }
    }
}

#[cfg(test)]
pub(super) fn reset_for_test() {
    TENURING_SURVIVALS.with(|s| s.set(GC_TENURING_SURVIVALS_MAX));
    RAISE_STREAK.with(|s| s.set(0));
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: usize = 1024 * 1024;

    #[test]
    fn target_formula_matches_projected_occupancy() {
        // Largest S with (S-1) * influx <= desired.
        assert_eq!(compute_target_survivals(0, MB), 4);
        assert_eq!(compute_target_survivals(20 * 1024, MB), 4); // churn-like
        assert_eq!(compute_target_survivals(MB / 3 - 1, MB), 4);
        assert_eq!(compute_target_survivals(MB / 2, MB), 3);
        assert_eq!(compute_target_survivals(MB, MB), 2);
        assert_eq!(compute_target_survivals(MB + MB / 20, MB), 1); // tree-like
        assert_eq!(compute_target_survivals(16 * MB, MB), 1); // retain-like
    }

    #[test]
    fn drops_immediately_and_rises_debounced() {
        reset_for_test();
        let desired = desired_survivor_bytes();
        assert_eq!(tenuring_survivals(), 4);

        // Heavy influx: instant drop to 1.
        retune_after_scavenge(desired * 2);
        assert_eq!(tenuring_survivals(), 1);

        // One quiet cycle: no rise yet (debounce).
        retune_after_scavenge(0);
        assert_eq!(tenuring_survivals(), 1);
        // Second quiet cycle: rise by exactly one step, not to the target.
        retune_after_scavenge(0);
        assert_eq!(tenuring_survivals(), 2);

        // Heavy again: streak resets and threshold drops straight back.
        retune_after_scavenge(desired * 2);
        assert_eq!(tenuring_survivals(), 1);

        // Sustained quiet recovers to the ceiling two cycles per step.
        for _ in 0..6 {
            retune_after_scavenge(0);
        }
        assert_eq!(tenuring_survivals(), 4);
        reset_for_test();
    }

    #[test]
    fn steady_heavy_influx_is_a_fixed_point() {
        reset_for_test();
        let heavy = desired_survivor_bytes() + desired_survivor_bytes() / 16;
        for _ in 0..10 {
            retune_after_scavenge(heavy);
            assert_eq!(tenuring_survivals(), 1);
        }
        reset_for_test();
    }
}
