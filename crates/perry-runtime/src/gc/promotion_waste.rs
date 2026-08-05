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
//!   OFF-arm behaviour that is optimal for this regime). The release
//!   signal is the post-full young-live trajectory — see
//!   [`note_full_collection_outcome`] — which stays measurable without
//!   promoting anything.
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
    /// Post-full young live bytes at the moment the veto engaged — the
    /// reference for the retain-like release condition.
    static YOUNG_LIVE_AT_VETO: Cell<usize> = const { Cell::new(0) };
}

/// Don't judge waste on less than this much promotion — a few stray
/// promoted objects dying is not a regime signal.
const WASTE_SUBSTANTIAL_BYTES: usize = 8 * 1024 * 1024;

/// Engage the veto when at least this share of a substantial promotion
/// window died by the next full.
const WASTE_VETO_DIED_PCT: usize = 50;

/// Release the veto when post-full young live bytes have grown past this
/// multiple of their level at engagement: the young generation is
/// ACCUMULATING (retain-like permanence), so promotion pays again. A
/// live-set-bound workload (tree-like) oscillates flat and never trips it.
const RELEASE_GROWTH_NUM: usize = 3;
const RELEASE_GROWTH_DEN: usize = 2;

pub(super) fn note_promoted_bytes(bytes: usize) {
    if bytes != 0 {
        PROMOTED_SINCE_FULL.with(|c| c.set(c.get().saturating_add(bytes)));
    }
}

/// Fold one full collection's outcome into the loop. `old_live_now` /
/// `old_live_prev_baseline` are live-pressure old-gen readings (in-use
/// minus reusable holes) after this full and after the previous one;
/// `young_live_now` is post-full young in-use (Eden + active survivor).
///
/// While vetoed, no promotion happens, so the waste ratio cannot refresh —
/// the release signal is the post-full young-live TRAJECTORY instead,
/// which stays measurable at zero cost:
/// - tiny young live ⇒ churn-like, promotion is moot either way: release;
/// - young live grown past 1.5× its level at engagement ⇒ the young
///   generation is accumulating permanent data (retain-like): release, so
///   the next scavenge compacts it into old-gen;
/// - flat / oscillating (live-set-bound, tree-like) ⇒ stay vetoed. An
///   earlier draft probed by letting every 8th scavenge run instead: each
///   probe re-promoted the whole live set, recreating a recurring ~30 MB
///   old-gen spike (peak RSS is a high-water metric — one probe per
///   period pins it) and paying promotion + drain wall time.
pub(super) fn note_full_collection_outcome(
    old_live_now: usize,
    old_live_prev_baseline: usize,
    young_live_now: usize,
) {
    let promoted = PROMOTED_SINCE_FULL.with(|c| c.replace(0));
    if PROMOTION_WASTE_VETO.with(Cell::get) && promoted < WASTE_SUBSTANTIAL_BYTES {
        // Vetoed and no fresh promotion evidence: evaluate release from the
        // young-live trajectory.
        let at_veto = YOUNG_LIVE_AT_VETO.with(Cell::get);
        let release = young_live_now < WASTE_SUBSTANTIAL_BYTES
            || young_live_now.saturating_mul(RELEASE_GROWTH_DEN)
                > at_veto.saturating_mul(RELEASE_GROWTH_NUM);
        if release {
            PROMOTION_WASTE_VETO.with(|v| v.set(false));
            if std::env::var_os("PERRY_GC_DIAG").is_some() {
                eprintln!(
                    "[gc-promotion-waste] veto released (young_live={} at_veto={})",
                    young_live_now, at_veto
                );
            }
        }
        return;
    }
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
    if veto && !was {
        YOUNG_LIVE_AT_VETO.with(|c| c.set(young_live_now));
    }
    if was != veto && std::env::var_os("PERRY_GC_DIAG").is_some() {
        eprintln!(
            "[gc-promotion-waste] veto {} (promoted={} retained={} died={}% young_live={})",
            if veto { "ENGAGED" } else { "released" },
            promoted,
            retained,
            died.saturating_mul(100) / promoted.max(1),
            young_live_now
        );
    }
}

/// True when the next arena-pressure minor should run as an in-place full
/// instead of a promoting scavenge. Pure read; release happens at full
/// collections via [`note_full_collection_outcome`].
pub(super) fn promotion_waste_full_escalation_due() -> bool {
    PROMOTION_WASTE_VETO.with(Cell::get)
}

#[cfg(test)]
pub(super) fn promotion_waste_reset_for_test() {
    PROMOTED_SINCE_FULL.with(|c| c.set(0));
    PROMOTION_WASTE_VETO.with(|v| v.set(false));
    YOUNG_LIVE_AT_VETO.with(|c| c.set(0));
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
    fn wasted_promotion_engages_veto() {
        promotion_waste_reset_for_test();
        // 20 MB promoted, old-gen retained only 1 MB of it by the next full.
        note_promoted_bytes(20 * MB);
        note_full_collection_outcome(3 * MB, 2 * MB, 30 * MB);
        assert!(promotion_waste_veto_for_test());
        assert!(promotion_waste_full_escalation_due());
        promotion_waste_reset_for_test();
    }

    #[test]
    fn retained_promotion_keeps_scavenging() {
        promotion_waste_reset_for_test();
        // retain.ts shape: everything promoted is still alive at the full.
        note_promoted_bytes(40 * MB);
        note_full_collection_outcome(45 * MB, 2 * MB, 10 * MB);
        assert!(!promotion_waste_veto_for_test());
        assert!(!promotion_waste_full_escalation_due());
        promotion_waste_reset_for_test();
    }

    #[test]
    fn small_promotion_windows_carry_no_signal() {
        promotion_waste_reset_for_test();
        note_promoted_bytes(MB);
        note_full_collection_outcome(0, 0, 30 * MB);
        assert!(!promotion_waste_veto_for_test());
        promotion_waste_reset_for_test();
    }

    #[test]
    fn flat_young_live_stays_vetoed_growth_releases() {
        promotion_waste_reset_for_test();
        note_promoted_bytes(20 * MB);
        note_full_collection_outcome(2 * MB, MB, 30 * MB);
        assert!(promotion_waste_veto_for_test());
        // tree.ts shape: post-full young live oscillates flat — vetoed.
        for young in [28, 32, 30, 34, 29] {
            note_full_collection_outcome(2 * MB, 2 * MB, young * MB);
            assert!(
                promotion_waste_veto_for_test(),
                "flat young must stay vetoed"
            );
        }
        // retain-like accumulation: young grows past 1.5× the engagement level.
        note_full_collection_outcome(2 * MB, 2 * MB, 50 * MB);
        assert!(
            !promotion_waste_veto_for_test(),
            "young-live growth past 1.5x must release the veto"
        );
        promotion_waste_reset_for_test();
    }

    #[test]
    fn tiny_young_live_releases_the_veto() {
        promotion_waste_reset_for_test();
        note_promoted_bytes(20 * MB);
        note_full_collection_outcome(2 * MB, MB, 30 * MB);
        assert!(promotion_waste_veto_for_test());
        // churn-like phase: nothing worth promoting either way.
        note_full_collection_outcome(2 * MB, 2 * MB, MB);
        assert!(!promotion_waste_veto_for_test());
        promotion_waste_reset_for_test();
    }
}
