//! GC progress contract: pause budgets per progress kind (split from
//! policy.rs for the per-file size gate). Work-unit budgets and soft pause
//! targets for the budgeted stepper, plus the debt-pacing gain constant used
//! by `gc_mutator_assist_scaled_work_units` (policy.rs).

/// Hard work budget for ordinary automatic GC steps once the collector is
/// split into resumable phases.
pub const GC_NORMAL_INCREMENTAL_WORK_UNITS: usize = 2_048;
/// Soft telemetry target for ordinary automatic GC steps.
pub const GC_NORMAL_INCREMENTAL_SOFT_PAUSE_US: u64 = 2_000;
/// BASE work budget for allocation-side mutator assist steps. The actual
/// per-assist budget is debt-scaled (`gc_mutator_assist_scaled_work_units`):
/// this constant alone is only enough when the collector is keeping up.
pub const GC_MUTATOR_ASSIST_WORK_UNITS: usize = 256;
/// Soft telemetry target for allocation-side mutator assist steps.
pub const GC_MUTATOR_ASSIST_SOFT_PAUSE_US: u64 = 500;
/// Debt-proportional assist pacing: one extra work unit per this many bytes
/// of arena debt (allocation past the armed trigger). This is the gain of a
/// proportional controller whose equilibrium debt scales as
/// sqrt(cycle_work × gain⁻¹): measured on a 10M-allocation churn loop, a
/// 1024-bytes-per-unit gain left cycles spanning ~300 MB of allocation
/// (pct_freed 156-190% in the re-arm DIAG) and RSS at 3.5× the synchronous
/// collector's. At 64 bytes per unit the same loop completes cycles within
/// ~its trigger step and RSS lands near parity. When the collector is
/// keeping up (debt ≈ 0) the budget stays at the base, so low-latency
/// workloads never see the scaled assists.
pub const GC_ASSIST_DEBT_BYTES_PER_WORK_UNIT: u64 = 32;

/// COMPLETION GUARANTEE (#6978): the most host safepoints one budgeted cycle
/// may span on the ordinary bounded budget before the safepoint finishes it
/// outright.
///
/// The budgeted stepper has a *budget* but no *completion guarantee*, and it
/// is driven by exactly two things: allocation-point mutator assists
/// (`gc_check_trigger`) and host safepoints (`gc_runtime_safepoint`, called
/// from the microtask checkpoint and the stdlib pump). A program that stops
/// allocating stops driving it. Measured on `test_gap_repsel_canonical_i32`
/// under `PERRY_GC_HEAP_LIMIT=8` (release, macOS arm64): `gc_check_trigger`
/// runs **twice in the whole process** — the second call arms an `ArenaBytes`
/// cycle and pays one 256-unit assist, and no allocation-point opportunity
/// ever comes again. The host safepoints that follow each paid the fixed
/// `GC_NORMAL_INCREMENTAL_WORK_UNITS` slice, which got the cycle through
/// `BuildValidPointerSet` and one step into `RootScan` before the process
/// exited. Note a fatter per-step budget cannot rescue this on its own: a
/// cycle has seven resumable phases and one `step()` advances at most one of
/// them, so completion needs a bounded *number of calls*, not more units.
///
/// A PARKED CYCLE IS NOT INERT. Until it completes:
///   * nothing is reclaimed and the arming trigger is never re-baselined;
///   * every subsequent allocation is born BLACK (`gc_birth_extra_flags`), so
///     the parked cycle can never collect it;
///   * the incremental mark barrier stays armed on every store; and
///   * `gc_safepoint_moving_minor` — the precise-root copying minor at the
///     outermost microtask boundary — returns early on
///     `gc_budgeted_cycle_active()`, so the parked cycle also disables the
///     collector's own moving path for the rest of the process.
/// Net effect on a compiled program in the shipped configuration: ZERO
/// completed collections.
///
/// A host safepoint is where the mutator has yielded and the JS stack has
/// unwound — the cheapest and safest point in the process to finish a
/// collection. So: a cycle may span this many host safepoints on the ordinary
/// bounded budget; at the next one the safepoint drives it to completion.
/// Measured cost on the corpus member that actually collects
/// (`test_gap_repsel_gc_stress`, `--pressure 8`): its cycles complete on
/// mutator assists alone (20 / 25 / 35 assist steps, **zero** host
/// safepoints), so a healthy allocating workload never reaches this limit and
/// pays nothing. The bound binds only when the mutator has stopped driving
/// the collector — precisely when parking forever is the alternative.
///
/// Kill switch: `PERRY_GC_SAFEPOINT_FINISH=0` (also `off` / `false`).
pub const GC_CYCLE_HOST_SAFEPOINT_LIMIT: u32 = 2;

std::thread_local! {
    /// How many host safepoints the currently-active budgeted cycle has
    /// already been offered. Reset whenever a safepoint finds no active cycle
    /// — which covers completion through every path (mutator assist, host
    /// safepoint, the drain before a synchronous collection).
    static GC_CYCLE_HOST_SAFEPOINTS: std::cell::Cell<u32> =
        const { std::cell::Cell::new(0) };
}

/// Kill switch for the #6978 completion guarantee. Default ON; `0` / `off` /
/// `false` restores the pre-fix behaviour, where every host safepoint takes
/// one bounded step and a cycle nobody drives parks for the life of the
/// process.
fn gc_safepoint_finish_enabled() -> bool {
    static CACHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        !matches!(
            std::env::var("PERRY_GC_SAFEPOINT_FINISH").as_deref(),
            Ok("0") | Ok("off") | Ok("false")
        )
    })
}

/// Count this host safepoint against the active budgeted cycle and report
/// whether the cycle has now outlived `GC_CYCLE_HOST_SAFEPOINT_LIMIT` of them
/// — i.e. whether this safepoint must finish it instead of taking another
/// bounded slice.
pub(super) fn gc_host_safepoint_starvation_due() -> bool {
    if !super::gc_budgeted_cycle_active() || !gc_safepoint_finish_enabled() {
        gc_reset_host_safepoint_starvation();
        return false;
    }
    let seen = GC_CYCLE_HOST_SAFEPOINTS.with(|seen| {
        let next = seen.get().saturating_add(1);
        seen.set(next);
        next
    });
    seen > GC_CYCLE_HOST_SAFEPOINT_LIMIT
}

pub(super) fn gc_reset_host_safepoint_starvation() {
    GC_CYCLE_HOST_SAFEPOINTS.with(|seen| seen.set(0));
}

/// Runtime-visible classification for GC progress.
///
/// Only `NormalIncremental` and `MutatorAssist` satisfy the low-pause
/// invariant today defined by this contract: bounded by work units, not heap
/// size. Explicit synchronous work and emergency full collections are allowed
/// to be unbounded only because they are separately requested or separately
/// reported.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GcProgressKind {
    NormalIncremental,
    MutatorAssist,
    ExplicitSynchronous,
    ExplicitFull,
    EmergencyFull,
    LegacySynchronous,
}

impl GcProgressKind {
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NormalIncremental => "normal_incremental",
            Self::MutatorAssist => "mutator_assist",
            Self::ExplicitSynchronous => "explicit_synchronous",
            Self::ExplicitFull => "explicit_full",
            Self::EmergencyFull => "emergency_full",
            Self::LegacySynchronous => "legacy_synchronous",
        }
    }

    #[inline]
    pub const fn is_budgeted(self) -> bool {
        matches!(self, Self::NormalIncremental | Self::MutatorAssist)
    }

    #[inline]
    pub const fn report_class(self) -> &'static str {
        match self {
            Self::NormalIncremental | Self::MutatorAssist => "ordinary_budgeted",
            Self::ExplicitSynchronous | Self::ExplicitFull => "explicit",
            Self::EmergencyFull => "emergency",
            Self::LegacySynchronous => "legacy",
        }
    }
}

/// Hard work-unit limit plus a soft pause target for telemetry.
///
/// `None` means the path is intentionally unbounded and must be labeled by its
/// `GcProgressKind`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GcPauseBudget {
    pub work_units: Option<usize>,
    pub pause_us: Option<u64>,
}

impl GcPauseBudget {
    #[inline]
    pub const fn bounded(work_units: usize, pause_us: u64) -> Self {
        Self {
            work_units: Some(work_units),
            pause_us: Some(pause_us),
        }
    }

    #[inline]
    pub const fn unbounded() -> Self {
        Self {
            work_units: None,
            pause_us: None,
        }
    }

    #[inline]
    pub const fn is_bounded(self) -> bool {
        self.work_units.is_some()
    }
}

/// GC progress policy exposed to runtime and trace consumers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GcProgressContract {
    pub normal_step_budget: GcPauseBudget,
    pub assist_budget: GcPauseBudget,
    pub explicit_synchronous_policy: GcPauseBudget,
    pub explicit_full_policy: GcPauseBudget,
    pub emergency_policy: GcPauseBudget,
}

impl GcProgressContract {
    #[inline]
    pub const fn budget_for(self, kind: GcProgressKind) -> GcPauseBudget {
        match kind {
            GcProgressKind::NormalIncremental => self.normal_step_budget,
            GcProgressKind::MutatorAssist => self.assist_budget,
            GcProgressKind::ExplicitSynchronous => self.explicit_synchronous_policy,
            GcProgressKind::ExplicitFull => self.explicit_full_policy,
            GcProgressKind::EmergencyFull => self.emergency_policy,
            GcProgressKind::LegacySynchronous => GcPauseBudget::unbounded(),
        }
    }
}

impl Default for GcProgressContract {
    fn default() -> Self {
        Self {
            normal_step_budget: GcPauseBudget::bounded(
                GC_NORMAL_INCREMENTAL_WORK_UNITS,
                GC_NORMAL_INCREMENTAL_SOFT_PAUSE_US,
            ),
            assist_budget: GcPauseBudget::bounded(
                GC_MUTATOR_ASSIST_WORK_UNITS,
                GC_MUTATOR_ASSIST_SOFT_PAUSE_US,
            ),
            explicit_synchronous_policy: GcPauseBudget::unbounded(),
            explicit_full_policy: GcPauseBudget::unbounded(),
            emergency_policy: GcPauseBudget::unbounded(),
        }
    }
}

/// Return Perry's process-wide GC progress contract.
pub fn gc_progress_contract() -> GcProgressContract {
    GcProgressContract::default()
}
