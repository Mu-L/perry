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
//! The occupancy rule alone is not sufficient: it optimises survivor
//! *space*, not copy *work*. A workload whose influx sits just under
//! `desired` (tree.ts measures 1,048,536 B against the 1,048,576 B
//! default — 40 bytes under) settles at S=2 and still copies every
//! surviving byte exactly once for nothing, because 100% of each cohort
//! survives its survivor round and gets promoted a cycle later anyway.
//! The **survival-rate lock** closes that: when last cycle's survivor
//! intake (`copied_bytes`) was substantial and ≥90% of it came back out
//! alive this cycle (`survivor_live_bytes`), the aging round demonstrably
//! filters nothing, so the threshold locks to 1 (promote on first copy)
//! until the influx goes quiet. The lock's exit signal — influx below
//! `desired/4` for two consecutive cycles — stays measurable while
//! locked, unlike survivor occupancy, which is zero at S=1 and would
//! leave the loop blind.
//!
//! There is no env knob here (see CLAUDE.md's GC knob kill-policy): the
//! loop is always on, and its neutral state — influx below `desired`,
//! survivor cohorts that die in place — computes S=4, which is
//! bit-for-bit the previous fixed behaviour.

use super::*;

/// Ceiling and power-on value: the previous fixed threshold.
pub(super) const GC_TENURING_SURVIVALS_MAX: u8 = GC_COPY_PROMOTION_SURVIVALS;

/// Consecutive cycles the computed target must exceed the current threshold
/// before it is raised (by one step).
const RAISE_DEBOUNCE_CYCLES: u8 = 2;

thread_local! {
    static TENURING_SURVIVALS: Cell<u8> = const { Cell::new(GC_TENURING_SURVIVALS_MAX) };
    static RAISE_STREAK: Cell<u8> = const { Cell::new(0) };
    /// Survival-rate lock: promote-on-first-copy until influx goes quiet.
    static PROMOTE_LOCK: Cell<bool> = const { Cell::new(false) };
    static UNLOCK_STREAK: Cell<u8> = const { Cell::new(0) };
    /// Bytes the previous copying minor put into the to-survivor space —
    /// the denominator of this cycle's survival rate.
    static PREV_COPIED_BYTES: Cell<usize> = const { Cell::new(0) };
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
/// of Eden, whether copied to a survivor space or promoted);
/// `copied_bytes` is what this cycle put into the to-survivor space;
/// `survivor_live_bytes` is what came back out of the from-survivor space
/// alive (numerator of the survival rate against the *previous* cycle's
/// `copied_bytes`).
pub(super) fn retune_after_scavenge(
    eden_live_bytes: usize,
    copied_bytes: usize,
    survivor_live_bytes: usize,
) {
    let desired = desired_survivor_bytes();
    let substantial = desired / 4;
    let prev_copied = PREV_COPIED_BYTES.with(|c| c.replace(copied_bytes));
    let current = TENURING_SURVIVALS.with(Cell::get);

    if PROMOTE_LOCK.with(Cell::get) {
        // Locked: stay at 1 while the influx stays substantial. The exit
        // signal is the influx itself — measurable every cycle, unlike
        // survivor occupancy, which is identically zero at S=1.
        if eden_live_bytes < substantial {
            let streak = UNLOCK_STREAK.with(|s| s.get()).saturating_add(1);
            if streak >= RAISE_DEBOUNCE_CYCLES {
                PROMOTE_LOCK.with(|l| l.set(false));
                UNLOCK_STREAK.with(|s| s.set(0));
                RAISE_STREAK.with(|s| s.set(0));
                // Resume the ladder one step up rather than snapping to the
                // ceiling; the normal debounced rise takes it the rest of
                // the way if the workload stays quiet.
                set_survivals(current, 2, eden_live_bytes, "unlock");
            } else {
                UNLOCK_STREAK.with(|s| s.set(streak));
            }
        } else {
            UNLOCK_STREAK.with(|s| s.set(0));
        }
        return;
    }

    // Survival-rate lock: last cycle's survivor intake was substantial and
    // (nearly) all of it came back out alive, so the aging round filters
    // nothing — every copied byte is a byte that will be promoted anyway.
    if prev_copied >= substantial && survivor_live_bytes.saturating_mul(10) >= prev_copied * 9 {
        PROMOTE_LOCK.with(|l| l.set(true));
        UNLOCK_STREAK.with(|s| s.set(0));
        RAISE_STREAK.with(|s| s.set(0));
        set_survivals(current, 1, eden_live_bytes, "lock");
        return;
    }

    let target = compute_target_survivals(eden_live_bytes, desired);
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
    set_survivals(current, next, eden_live_bytes, "occupancy");
}

fn set_survivals(current: u8, next: u8, eden_live_bytes: usize, why: &str) {
    if next == current {
        return;
    }
    TENURING_SURVIVALS.with(|s| s.set(next));
    if std::env::var_os("PERRY_GC_DIAG").is_some() {
        eprintln!(
            "[gc-tenuring] survivals {} -> {} ({why}, eden_live_bytes={} desired={})",
            current,
            next,
            eden_live_bytes,
            desired_survivor_bytes()
        );
    }
}

#[cfg(test)]
pub(super) fn reset_for_test() {
    TENURING_SURVIVALS.with(|s| s.set(GC_TENURING_SURVIVALS_MAX));
    RAISE_STREAK.with(|s| s.set(0));
    PROMOTE_LOCK.with(|l| l.set(false));
    UNLOCK_STREAK.with(|s| s.set(0));
    PREV_COPIED_BYTES.with(|c| c.set(0));
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
        retune_after_scavenge(desired * 2, 0, 0);
        assert_eq!(tenuring_survivals(), 1);

        // One quiet cycle: no rise yet (debounce).
        retune_after_scavenge(0, 0, 0);
        assert_eq!(tenuring_survivals(), 1);
        // Second quiet cycle: rise by exactly one step, not to the target.
        retune_after_scavenge(0, 0, 0);
        assert_eq!(tenuring_survivals(), 2);

        // Heavy again: streak resets and threshold drops straight back.
        retune_after_scavenge(desired * 2, 0, 0);
        assert_eq!(tenuring_survivals(), 1);

        // Sustained quiet recovers to the ceiling two cycles per step.
        for _ in 0..6 {
            retune_after_scavenge(0, 0, 0);
        }
        assert_eq!(tenuring_survivals(), 4);
        reset_for_test();
    }

    #[test]
    fn steady_heavy_influx_is_a_fixed_point() {
        reset_for_test();
        let heavy = desired_survivor_bytes() + desired_survivor_bytes() / 16;
        for _ in 0..10 {
            retune_after_scavenge(heavy, 0, 0);
            assert_eq!(tenuring_survivals(), 1);
        }
        reset_for_test();
    }

    #[test]
    fn survival_rate_lock_breaks_a_saturated_pipeline() {
        reset_for_test();
        let d = desired_survivor_bytes();
        // tree.ts steady state: influx sits JUST under desired (occupancy
        // alone settles at S=2), the survivor space holds 3 cohorts, and
        // 100% of every intake comes back out alive.
        let influx = d - 64;
        retune_after_scavenge(influx, 3 * d, 3 * d);
        assert_eq!(
            tenuring_survivals(),
            2,
            "first cycle has no prior intake to rate, so occupancy decides"
        );
        retune_after_scavenge(influx, 3 * d, 3 * d);
        assert_eq!(
            tenuring_survivals(),
            1,
            "a substantial intake that fully survives its round must lock promote-on-first-copy"
        );
        // The lock holds through occupancy readings that would say S=2.
        for _ in 0..5 {
            retune_after_scavenge(influx, 0, 0);
            assert_eq!(tenuring_survivals(), 1);
        }
        // Quiet influx exits the lock after the debounce, resuming the
        // ladder one step up rather than snapping to the ceiling.
        retune_after_scavenge(0, 0, 0);
        assert_eq!(tenuring_survivals(), 1);
        retune_after_scavenge(0, 0, 0);
        assert_eq!(tenuring_survivals(), 2);
        for _ in 0..4 {
            retune_after_scavenge(0, 0, 0);
        }
        assert_eq!(tenuring_survivals(), 4);
        reset_for_test();
    }

    #[test]
    fn dying_survivor_cohorts_do_not_lock() {
        reset_for_test();
        let d = desired_survivor_bytes();
        // Medium-lived objects: a substantial intake of which only half
        // survives its survivor round. Aging is filtering — the lock must
        // stay out and the occupancy ladder must decide.
        for _ in 0..6 {
            retune_after_scavenge(d / 2, d / 2, d / 4);
            assert!(
                tenuring_survivals() >= 3,
                "a cohort that dies in the survivor space must keep aging (got {})",
                tenuring_survivals()
            );
        }
        reset_for_test();
    }
}
