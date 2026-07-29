use super::super::*;
use super::support::*;

fn reset_old_reclaim_pressure() {
    let old_in_use = crate::arena::old_gen_in_use_bytes();
    GC_LAST_OLD_RECLAIM_IN_USE_BYTES.with(|bytes| bytes.set(old_in_use));
    GC_OLD_RECLAIM_PENDING.with(|pending| pending.set(false));
}

fn live_test_string(bytes: &'static [u8]) -> usize {
    crate::string::js_string_from_bytes(bytes.as_ptr(), bytes.len() as u32) as usize
}

fn make_arena_pressure(trigger_guard: &GcTriggerThresholdTestGuard, live_bytes: &'static [u8]) {
    let live = live_test_string(live_bytes);
    js_shadow_slot_set(0, string_bits(live));
    for _ in 0..6_000 {
        let _ = young_leaf();
    }
    trigger_guard.make_arena_trigger_due();
}

fn complete_host_safepoint_cycle() -> JsGcStepResult {
    for _ in 0..500_000 {
        let result = gc_runtime_safepoint();
        match result.status {
            JS_GC_STEP_STATUS_ACTIVE => continue,
            JS_GC_STEP_STATUS_COMPLETED => return result,
            other => panic!("host safepoint cycle stopped before completion: status {other}"),
        }
    }
    panic!("host safepoint cycle did not complete within step limit");
}

struct SuppressGcGuard;

impl SuppressGcGuard {
    fn enter() -> Self {
        gc_suppress();
        Self
    }
}

impl Drop for SuppressGcGuard {
    fn drop(&mut self) {
        gc_unsuppress();
    }
}

struct UnsafeZoneGuard;

impl UnsafeZoneGuard {
    fn enter() -> Self {
        js_gc_enter_unsafe_zone();
        Self
    }
}

impl Drop for UnsafeZoneGuard {
    fn drop(&mut self) {
        js_gc_exit_unsafe_zone();
    }
}

struct RootLockGuard;

impl RootLockGuard {
    fn enter() -> Self {
        super::super::roots::enter_gc_root_lock();
        Self
    }
}

impl Drop for RootLockGuard {
    fn drop(&mut self) {
        super::super::roots::exit_gc_root_lock();
    }
}

#[test]
fn no_pressure_runtime_safepoint_reports_idle_without_starting_cycle() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let _trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();

    let before = gc_collection_count();
    let result = gc_runtime_safepoint();

    assert_eq!(result.status, JS_GC_STEP_STATUS_IDLE);
    assert_eq!(result.active, 0);
    assert_eq!(result.completed, 0);
    assert_eq!(gc_collection_count(), before);
}

#[test]
fn arena_pressure_runtime_safepoint_starts_bounded_normal_work() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();
    make_arena_pressure(&trigger_guard, b"host_safepoint_live");

    let before = gc_collection_count();
    let result = gc_runtime_safepoint();

    assert_eq!(result.status, JS_GC_STEP_STATUS_ACTIVE);
    assert_eq!(result.active, 1);
    assert_eq!(result.completed, 0);
    assert_eq!(result.collection_kind, GcCollectionKind::Minor.ffi_code());
    assert_eq!(result.trigger_kind, GcTriggerKind::ArenaBytes.ffi_code());
    assert!(result.arena_debt_bytes > 0);
    assert_eq!(
        gc_collection_count(),
        before,
        "one scheduler safepoint should not complete a monolithic collection"
    );
}

#[test]
fn repeated_runtime_safepoints_complete_cycle_rebaseline_debt_and_preserve_roots() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();
    make_arena_pressure(&trigger_guard, b"host_safepoint_drain_live");

    let first = gc_runtime_safepoint();
    assert_eq!(first.status, JS_GC_STEP_STATUS_ACTIVE);

    let completed = complete_host_safepoint_cycle();
    assert_eq!(completed.status, JS_GC_STEP_STATUS_COMPLETED);
    assert_eq!(completed.active, 0);
    assert_eq!(completed.completed, 1);
    assert_eq!(completed.arena_debt_bytes, 0);
    assert!(
        GC_NEXT_TRIGGER_BYTES.with(|trigger| trigger.get()) > crate::arena::arena_total_bytes(),
        "completed safepoint cycle should rebaseline the arena trigger"
    );

    let live_after = (js_shadow_slot_get(0) & POINTER_MASK) as *const crate::StringHeader;
    unsafe {
        assert_string_bytes(live_after, b"host_safepoint_drain_live");
    }
}

/// #6978: a budgeted cycle that nothing drives must still complete.
///
/// The stepper has a budget but no completion guarantee. Its only two drivers
/// are allocation-point mutator assists and host safepoints, so a program
/// that stops allocating parks its cycle — measured on a compiled program,
/// `gc_check_trigger` ran twice in the whole process, and the seven host
/// safepoints that followed each paid one fixed `NormalIncremental` slice and
/// left the cycle in `RootScan`. Parked is not inert: nothing is reclaimed,
/// the arming trigger is never re-baselined, every later allocation is born
/// black, and `gc_safepoint_moving_minor` early-returns on
/// `gc_budgeted_cycle_active()` for the rest of the process.
///
/// So the host safepoint carries the guarantee: a cycle may span
/// `GC_CYCLE_HOST_SAFEPOINT_LIMIT` safepoints on the ordinary bounded budget,
/// and the next one finishes it. Both halves are asserted — the early
/// safepoints must NOT finish it (that would make incremental mode
/// synchronous), and the one past the limit must, WITHOUT any further
/// allocation to assist it.
#[test]
fn a_cycle_that_nothing_drives_is_finished_within_a_bounded_number_of_host_safepoints() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();
    make_arena_pressure(&trigger_guard, b"host_safepoint_starved_live");

    let before = gc_collection_count();
    let started = gc_runtime_safepoint();
    assert_eq!(started.status, JS_GC_STEP_STATUS_ACTIVE);
    for offered in 1..=GC_CYCLE_HOST_SAFEPOINT_LIMIT {
        let result = gc_runtime_safepoint();
        assert_eq!(
            result.status, JS_GC_STEP_STATUS_ACTIVE,
            "host safepoint {offered} of {GC_CYCLE_HOST_SAFEPOINT_LIMIT} must stay an \
             ordinary bounded step -- finishing early would make incremental mode synchronous"
        );
        assert_eq!(gc_collection_count(), before);
    }

    // No allocation has happened since the cycle armed, so no mutator assist
    // can ever come. This safepoint is the completion guarantee.
    let completed = gc_runtime_safepoint();
    assert_eq!(
        completed.status, JS_GC_STEP_STATUS_COMPLETED,
        "a cycle offered more than {GC_CYCLE_HOST_SAFEPOINT_LIMIT} host safepoints without \
         completing must be finished by the safepoint, not parked"
    );
    assert_eq!(completed.completed, 1);
    assert_eq!(completed.active, 0);
    assert!(gc_collection_count() > before);
    assert_eq!(
        js_gc_step_status(std::ptr::null_mut()),
        JS_GC_STEP_STATUS_IDLE,
        "no budgeted cycle may remain parked after the completion guarantee fires"
    );
    assert!(
        GC_NEXT_TRIGGER_BYTES.with(|trigger| trigger.get()) > crate::arena::arena_total_bytes(),
        "the finished cycle must re-baseline the arming trigger like any other completion"
    );

    let live_after = (js_shadow_slot_get(0) & POINTER_MASK) as *const crate::StringHeader;
    unsafe {
        assert_string_bytes(live_after, b"host_safepoint_starved_live");
    }
}

/// #6978 follow-up: the host-safepoint allowance is charged PER CYCLE.
///
/// A mutator assist can finish one cycle and arm the next entirely between two
/// host safepoints. If the counter were only cleared when a safepoint happens
/// to observe an idle collector, the second cycle would inherit its
/// predecessor's spent allowance and be driven to completion on its very first
/// safepoint — silently synchronous. The FFI stepper stands in here for the
/// mutator assist: it drives cycles without going through
/// `gc_runtime_safepoint`, which is exactly the off-safepoint path.
#[test]
fn the_host_safepoint_allowance_is_charged_per_cycle_not_carried_across_cycles() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();

    // Cycle A: spend the whole safepoint allowance without completing it.
    make_arena_pressure(&trigger_guard, b"host_safepoint_cycle_a_live");
    assert_eq!(gc_runtime_safepoint().status, JS_GC_STEP_STATUS_ACTIVE);
    for _ in 0..GC_CYCLE_HOST_SAFEPOINT_LIMIT {
        assert_eq!(gc_runtime_safepoint().status, JS_GC_STEP_STATUS_ACTIVE);
    }

    // ...then finish A OFF-safepoint, and arm + start B off-safepoint too, so
    // no safepoint ever observes the collector idle in between.
    let mut status = JsGcStepResult::default();
    let mut finished_off_safepoint = false;
    for _ in 0..64 {
        if js_gc_step_work_units(u64::MAX, &mut status) != JS_GC_STEP_STATUS_ACTIVE {
            finished_off_safepoint = true;
            break;
        }
    }
    assert!(
        finished_off_safepoint,
        "cycle A should complete through the host-driven stepper"
    );
    make_arena_pressure(&trigger_guard, b"host_safepoint_cycle_b_live");
    assert_eq!(
        js_gc_step_work_units(1, &mut status),
        JS_GC_STEP_STATUS_ACTIVE,
        "cycle B should start off-safepoint, the way a mutator assist starts one"
    );

    // Cycle B must get its OWN allowance, not A's leftovers.
    for offered in 1..=GC_CYCLE_HOST_SAFEPOINT_LIMIT {
        assert_eq!(
            gc_runtime_safepoint().status,
            JS_GC_STEP_STATUS_ACTIVE,
            "safepoint {offered} of the SECOND cycle must still be an ordinary bounded \
             step -- the allowance is per cycle, not per process"
        );
    }
    assert_eq!(
        gc_runtime_safepoint().status,
        JS_GC_STEP_STATUS_COMPLETED,
        "and the guarantee must still fire for the second cycle"
    );

    let live_after = (js_shadow_slot_get(0) & POINTER_MASK) as *const crate::StringHeader;
    unsafe {
        assert_string_bytes(live_after, b"host_safepoint_cycle_b_live");
    }
}

#[test]
fn microtask_runner_tail_pays_bounded_safepoint_under_pressure() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();
    make_arena_pressure(&trigger_guard, b"host_safepoint_microtask_live");

    let before = gc_collection_count();
    let _ran = crate::promise::js_promise_run_microtasks();

    let mut status = JsGcStepResult::default();
    assert_eq!(js_gc_step_status(&mut status), JS_GC_STEP_STATUS_ACTIVE);
    assert_eq!(status.trigger_kind, GcTriggerKind::ArenaBytes.ffi_code());
    assert_eq!(gc_collection_count(), before);

    let completed = complete_host_safepoint_cycle();
    assert_eq!(completed.status, JS_GC_STEP_STATUS_COMPLETED);
}

#[test]
fn stdlib_pump_and_perry_poll_pay_debt_through_shared_scheduler_surfaces() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();
    make_arena_pressure(&trigger_guard, b"host_safepoint_stdlib_live");

    crate::stdlib_pump::js_run_stdlib_pump();

    let mut status = JsGcStepResult::default();
    assert_eq!(js_gc_step_status(&mut status), JS_GC_STEP_STATUS_ACTIVE);
    assert_eq!(status.trigger_kind, GcTriggerKind::ArenaBytes.ffi_code());
    let completed = complete_host_safepoint_cycle();
    assert_eq!(completed.status, JS_GC_STEP_STATUS_COMPLETED);

    make_arena_pressure(&trigger_guard, b"host_safepoint_poll_live");
    let _microtasks = crate::event_pump::perry_poll();

    assert_eq!(js_gc_step_status(&mut status), JS_GC_STEP_STATUS_ACTIVE);
    assert_eq!(status.trigger_kind, GcTriggerKind::ArenaBytes.ffi_code());
    let completed = complete_host_safepoint_cycle();
    assert_eq!(completed.status, JS_GC_STEP_STATUS_COMPLETED);
}

#[test]
fn js_gc_safepoint_null_and_output_pointer_are_safe() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();

    assert_eq!(
        js_gc_safepoint(std::ptr::null_mut()),
        JS_GC_STEP_STATUS_IDLE
    );

    make_arena_pressure(&trigger_guard, b"host_safepoint_ffi_live");
    let mut result = JsGcStepResult::default();
    let status = js_gc_safepoint(&mut result);

    assert_eq!(status, result.status);
    assert_eq!(result.status, JS_GC_STEP_STATUS_ACTIVE);
    assert_eq!(result.active, 1);
    assert_eq!(result.completed, 0);
    assert!(result.arena_debt_bytes > 0);
    assert_eq!(result.trigger_kind, GcTriggerKind::ArenaBytes.ffi_code());
}

#[test]
fn unsafe_suppressed_and_root_locked_safepoints_skip_without_collecting() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();

    make_arena_pressure(&trigger_guard, b"host_safepoint_unsafe_live");
    let before = gc_collection_count();
    {
        let _unsafe_zone = UnsafeZoneGuard::enter();
        let result = gc_runtime_safepoint();
        assert_eq!(result.status, JS_GC_STEP_STATUS_SKIPPED);
        assert_eq!(result.active, 0);
        assert_eq!(gc_collection_count(), before);
    }

    {
        let _suppressed = SuppressGcGuard::enter();
        let result = gc_runtime_safepoint();
        assert_eq!(result.status, JS_GC_STEP_STATUS_SKIPPED);
        assert_eq!(result.active, 0);
        assert_eq!(gc_collection_count(), before);
    }

    {
        let _root_lock = RootLockGuard::enter();
        let result = gc_runtime_safepoint();
        assert_eq!(result.status, JS_GC_STEP_STATUS_SKIPPED);
        assert_eq!(result.active, 0);
        assert_eq!(gc_collection_count(), before);
    }
}

#[test]
fn host_safepoint_trace_reports_normal_incremental_budgeted_steps() {
    let _trace_guard = TestGcTraceCaptureGuard::force_enabled();
    let _guard = CopyingNurseryTestGuard::new(1);
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();
    make_arena_pressure(&trigger_guard, b"host_safepoint_trace_live");

    let completed = complete_host_safepoint_cycle();
    assert_eq!(completed.status, JS_GC_STEP_STATUS_COMPLETED);

    let event = take_test_last_gc_trace_json().expect("host safepoint completion should trace");
    assert_eq!(
        event["pause_budget"]["kind"].as_str(),
        Some("normal_incremental")
    );
    assert_eq!(
        event["pause_budget"]["class"].as_str(),
        Some("ordinary_budgeted")
    );

    let steps = event["pause_steps"]
        .as_array()
        .expect("host safepoint trace should include pause_steps");
    assert!(
        !steps.is_empty(),
        "host safepoint cycle should report ordinary pause steps"
    );
    for (index, step) in steps.iter().enumerate() {
        assert_eq!(
            step["budget"]["kind"].as_str(),
            Some("normal_incremental"),
            "pause_steps[{index}] should use the host safepoint budget kind"
        );
        assert_eq!(
            step["budget"]["class"].as_str(),
            Some("ordinary_budgeted"),
            "pause_steps[{index}] should stay ordinary budgeted"
        );
        assert_eq!(
            step["budget"]["ordinary_budgeted"].as_bool(),
            Some(true),
            "pause_steps[{index}] should count as ordinary budgeted work"
        );
        assert!(
            step["budget"]["within_soft_pause_target"]
                .as_bool()
                .is_some(),
            "pause_steps[{index}] should report soft pause target status"
        );
    }
}
