//! Promotion-waste feedback (#7438): stop promoting cohorts that die in
//! old-gen anyway.
//!
//! The adaptive tenuring lock (gc/tenuring.rs) answers "does the survivor
//! space filter anything?" — but a cohort that fully survives its survivor
//! round can still die shortly after PROMOTION, and on live-set-bound
//! workloads that promotes entire consecutive live sets into old-gen for
//! nothing. Measured on tree.ts: the startup transient promoted two whole
//! ~35 MB live trees back-to-back (each died within two collections),
//! spiking old-gen's committed high-water to 68 MB — the single term that
//! kept scavenge-on peak RSS at ~190 MB against the 102 MB scavenge-off
//! arm, whose steady state the collector converges to anyway (the K×
//! major-pacing escalation turns every steady-state collection into an
//! in-place full; only the transient's promoting minors slip through,
//! because the escalation baseline chases the growing live set).
//!
//! The loop mirrors the tenuring lock's shape — measured feedback, probing
//! exit, no env knob, neutral state identical to the previous behaviour:
//!
//! - Every copying minor accumulates its promoted bytes.
//! - Every full collection's baseline finisher compares the accumulated
//!   promotion against what old-gen actually RETAINED across the window
//!   (live-pressure basis, so #7443's reusable holes count as dead). A
//!   substantial promotion volume that mostly died engages the veto.
//! - While vetoed, arena-pressure minors escalate to in-place fulls (the
//!   OFF-arm behaviour that is optimal for this regime). Every
//!   [`VETO_PROBE_PERIOD`]-th escalation decision lets one scavenge
//!   through so the waste signal stays live — a workload that shifts to
//!   retain-like permanence (promoted cohorts stay alive) un-vetoes on the
//!   next full.
//!
//! Workload outcomes: tree.ts's transient collapses onto its own steady
//! state (old high-water ≈ one flush instead of two live sets); retain.ts
//! measures ~0% waste and keeps the compact-into-old behaviour its RSS
//! advantage depends on; churn never promotes, so the loop never engages.

use super::*;

thread_local! {
    /// Bytes promoted by copying minors since the last full collection's
    /// baseline finisher ran.
    static PROMOTED_SINCE_FULL: Cell<usize> = const { Cell::new(0) };
    static PROMOTION_WASTE_VETO: Cell<bool> = const { Cell::new(false) };
    static VETOED_DECISIONS: Cell<u32> = const { Cell::new(0) };
}

/// Don't judge waste on less than this much promotion — a few stray
/// promoted objects dying is not a regime signal.
const WASTE_SUBSTANTIAL_BYTES: usize = 8 * 1024 * 1024;

/// Engage the veto when at least this share of a substantial promotion
/// window died by the next full.
const WASTE_VETO_DIED_PCT: usize = 50;

/// Every Nth vetoed escalation decision lets the scavenge run anyway, so
/// the waste signal keeps refreshing (a permanently latched veto would be
/// blind to a phase change toward retain-like permanence).
const VETO_PROBE_PERIOD: u32 = 8;

pub(super) fn note_promoted_bytes(bytes: usize) {
    if bytes != 0 {
        PROMOTED_SINCE_FULL.with(|c| c.set(c.get().saturating_add(bytes)));
    }
}

/// Fold one full collection's outcome into the loop. `old_live_now` and
/// `old_live_prev_baseline` are live-pressure old-gen readings (in-use
/// minus reusable holes) after this full and after the previous one.
pub(super) fn note_full_collection_outcome(old_live_now: usize, old_live_prev_baseline: usize) {
    let promoted = PROMOTED_SINCE_FULL.with(|c| c.replace(0));
    if promoted < WASTE_SUBSTANTIAL_BYTES {
        // No substantial promotion this window: no evidence either way.
        return;
    }
    let retained = old_live_now
        .saturating_sub(old_live_prev_baseline)
        .min(promoted);
    let died = promoted - retained;
    let veto = died.saturating_mul(100) >= promoted.saturating_mul(WASTE_VETO_DIED_PCT);
    let was = PROMOTION_WASTE_VETO.with(|v| v.replace(veto));
    if !veto {
        VETOED_DECISIONS.with(|c| c.set(0));
    }
    if was != veto && std::env::var_os("PERRY_GC_DIAG").is_some() {
        eprintln!(
            "[gc-promotion-waste] veto {} (promoted={} retained={} died={}%)",
            if veto { "ENGAGED" } else { "released" },
            promoted,
            retained,
            died.saturating_mul(100) / promoted.max(1)
        );
    }
}

/// True when the next arena-pressure minor should run as an in-place full
/// instead of a promoting scavenge. Stateful: every [`VETO_PROBE_PERIOD`]-th
/// decision under an engaged veto returns false (the probe scavenge), so
/// call it once per collection decision, after cheaper gates.
pub(super) fn promotion_waste_full_escalation_due() -> bool {
    if !PROMOTION_WASTE_VETO.with(Cell::get) {
        return false;
    }
    let n = VETOED_DECISIONS.with(|c| c.get()).wrapping_add(1);
    VETOED_DECISIONS.with(|c| c.set(n));
    n % VETO_PROBE_PERIOD != 0
}

#[cfg(test)]
pub(super) fn promotion_waste_reset_for_test() {
    PROMOTED_SINCE_FULL.with(|c| c.set(0));
    PROMOTION_WASTE_VETO.with(|v| v.set(false));
    VETOED_DECISIONS.with(|c| c.set(0));
}

#[cfg(test)]
pub(super) fn promotion_waste_veto_for_test() -> bool {
    PROMOTION_WASTE_VETO.with(Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: usize = 1024 * 1024;

    #[test]
    fn wasted_promotion_engages_veto_and_probes() {
        promotion_waste_reset_for_test();
        // 20 MB promoted, old-gen retained only 1 MB of it by the next full.
        note_promoted_bytes(20 * MB);
        note_full_collection_outcome(3 * MB, 2 * MB);
        assert!(promotion_waste_veto_for_test());
        // Escalations are due, except every VETO_PROBE_PERIOD-th decision.
        let mut allowed = 0;
        for _ in 0..VETO_PROBE_PERIOD * 2 {
            if !promotion_waste_full_escalation_due() {
                allowed += 1;
            }
        }
        assert_eq!(allowed, 2, "exactly one probe scavenge per period");
        promotion_waste_reset_for_test();
    }

    #[test]
    fn retained_promotion_keeps_scavenging() {
        promotion_waste_reset_for_test();
        // retain.ts shape: everything promoted is still alive at the full.
        note_promoted_bytes(40 * MB);
        note_full_collection_outcome(45 * MB, 2 * MB);
        assert!(!promotion_waste_veto_for_test());
        assert!(!promotion_waste_full_escalation_due());
        promotion_waste_reset_for_test();
    }

    #[test]
    fn small_promotion_windows_carry_no_signal() {
        promotion_waste_reset_for_test();
        note_promoted_bytes(MB);
        note_full_collection_outcome(0, 0);
        assert!(!promotion_waste_veto_for_test());
        promotion_waste_reset_for_test();
    }

    #[test]
    fn probe_scavenge_whose_cohort_survives_releases_the_veto() {
        promotion_waste_reset_for_test();
        note_promoted_bytes(20 * MB);
        note_full_collection_outcome(2 * MB, MB);
        assert!(promotion_waste_veto_for_test());
        // The probe scavenge promotes a cohort that old-gen RETAINS.
        note_promoted_bytes(10 * MB);
        note_full_collection_outcome(12 * MB, 2 * MB);
        assert!(
            !promotion_waste_veto_for_test(),
            "a retained probe cohort must release the veto"
        );
        promotion_waste_reset_for_test();
    }
}
