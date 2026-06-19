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

fn budgeted_step_until_phase(target: GcCyclePhase) -> JsGcStepResult {
    let mut status = JsGcStepResult::default();
    for _ in 0..500_000 {
        let current = js_gc_step_status(&mut status);
        if current == JS_GC_STEP_STATUS_ACTIVE && status.phase == target.ffi_code() {
            return status;
        }
        let stepped = js_gc_step_work_units(1, &mut status);
        if stepped == JS_GC_STEP_STATUS_ACTIVE && status.phase == target.ffi_code() {
            return status;
        }
        assert_ne!(
            stepped, JS_GC_STEP_STATUS_COMPLETED,
            "budgeted cycle completed before reaching phase {target:?}"
        );
    }
    panic!("budgeted cycle did not reach phase {target:?}");
}

#[test]
fn arena_threshold_debt_starts_bounded_assist_without_monolithic_collection() {
    let _trace_guard = TestGcTraceCaptureGuard::force_enabled();
    let _guard = CopyingNurseryTestGuard::new(1);
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();

    let live = live_test_string(b"arena_debt_live");
    js_shadow_slot_set(0, string_bits(live));
    for _ in 0..(GC_MUTATOR_ASSIST_WORK_UNITS * 4) {
        let _ = young_leaf();
    }
    trigger_guard.make_arena_trigger_due();

    let before = gc_collection_count();
    gc_check_trigger();

    let mut status = JsGcStepResult::default();
    assert_eq!(
        js_gc_step_status(&mut status),
        JS_GC_STEP_STATUS_IDLE,
        "eligible arena pressure should complete during allocation-side copied-minor assist"
    );
    assert!(gc_collection_count() > before);
    let live_after = (js_shadow_slot_get(0) & POINTER_MASK) as *const crate::StringHeader;
    unsafe {
        assert_string_bytes(live_after, b"arena_debt_live");
    }
    assert!(
        GC_NEXT_TRIGGER_BYTES.with(|trigger| trigger.get()) > gc_nursery_trigger_bytes(),
        "completed arena debt cycle should rebaseline the heap goal"
    );
    let event = take_test_last_gc_trace_json().expect("arena debt copied-minor should trace");
    assert_eq!(event["collection_kind"].as_str(), Some("minor"));
    assert_eq!(event["trigger"]["kind"].as_str(), Some("arena_bytes"));
    assert_eq!(event["copying_nursery"]["eligible"].as_bool(), Some(true));
    assert_eq!(
        event["copying_nursery"]["fallback_reason"].as_str(),
        Some("none")
    );
}

#[test]
fn malloc_threshold_debt_reclaims_dead_churn_after_host_drain() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let _trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    let _barrier_guard = GeneratedWriteBarrierTestGuard::inactive();
    reset_old_reclaim_pressure();

    let live_malloc = gc_malloc(
        std::mem::size_of::<crate::closure::ClosureHeader>(),
        GC_TYPE_CLOSURE,
    );
    unsafe {
        init_test_closure(live_malloc);
    }
    js_shadow_slot_set(0, ptr_bits(live_malloc as usize));

    let churn_headers = allocate_dead_malloc_churn_headers(128);
    assert_eq!(
        tracked_malloc_headers_matching(&churn_headers),
        churn_headers.len()
    );
    let malloc_count = malloc_object_count();
    GC_NEXT_MALLOC_TRIGGER.with(|trigger| trigger.set(malloc_count.saturating_sub(1)));

    let before = gc_collection_count();
    gc_check_trigger();

    let mut status = JsGcStepResult::default();
    assert_eq!(js_gc_step_status(&mut status), JS_GC_STEP_STATUS_ACTIVE);
    assert_eq!(status.collection_kind, GcCollectionKind::Minor.ffi_code());
    assert_eq!(status.trigger_kind, GcTriggerKind::MallocCount.ffi_code());
    assert!(status.malloc_debt_objects > 0);
    assert_eq!(
        gc_collection_count(),
        before,
        "malloc pressure should be assisted, not synchronously collected"
    );

    let completed = complete_budgeted_gc_cycle();
    assert_eq!(completed.status, JS_GC_STEP_STATUS_COMPLETED);
    assert!(
        malloc_user_ptr_tracked(live_malloc),
        "live malloc root should survive the completed debt cycle"
    );
    assert_eq!(
        tracked_malloc_headers_matching(&churn_headers),
        0,
        "dead malloc churn should be reclaimed once debt is drained"
    );

    let survivors_after = malloc_object_count();
    let malloc_step_after = GC_MALLOC_COUNT_STEP.with(|step| step.get());
    assert_eq!(
        GC_NEXT_MALLOC_TRIGGER.with(|trigger| trigger.get()),
        survivors_after + malloc_step_after
    );
}

#[test]
fn active_cycle_gc_check_trigger_calls_pay_bounded_assist_work() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();

    let live = live_test_string(b"active_assist_live");
    js_shadow_slot_set(0, string_bits(live));
    for _ in 0..(GC_MUTATOR_ASSIST_WORK_UNITS * 8) {
        let _ = young_leaf();
    }
    trigger_guard.make_arena_trigger_due();

    let before = gc_collection_count();
    let mut status = JsGcStepResult::default();
    assert_eq!(
        js_gc_step_work_units(1, &mut status),
        JS_GC_STEP_STATUS_ACTIVE
    );
    assert_eq!(gc_collection_count(), before);

    gc_check_trigger();
    let step_after_assist = js_gc_step_status(&mut status);
    assert!(
        step_after_assist == JS_GC_STEP_STATUS_ACTIVE
            || step_after_assist == JS_GC_STEP_STATUS_IDLE,
        "active-cycle assist should leave the existing cycle active or complete it"
    );
    assert!(
        gc_collection_count() <= before + 1,
        "active-cycle assists must not start nested collections"
    );

    if step_after_assist == JS_GC_STEP_STATUS_ACTIVE {
        let completed = complete_budgeted_gc_cycle();
        assert_eq!(completed.status, JS_GC_STEP_STATUS_COMPLETED);
    }
    assert!(gc_collection_count() > before);
    let live_after = (js_shadow_slot_get(0) & POINTER_MASK) as *const crate::StringHeader;
    unsafe {
        assert_string_bytes(live_after, b"active_assist_live");
    }
}

#[test]
fn allocation_assists_advance_sliced_finalize_sweep_and_reclaim() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let _trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    let _barrier_guard = GeneratedWriteBarrierTestGuard::inactive();
    reset_old_reclaim_pressure();

    let live_malloc = gc_malloc(
        std::mem::size_of::<crate::closure::ClosureHeader>(),
        GC_TYPE_CLOSURE,
    );
    unsafe {
        init_test_closure(live_malloc);
    }
    js_shadow_slot_set(0, ptr_bits(live_malloc as usize));

    let churn_headers = allocate_dead_malloc_churn_headers(128);
    assert_eq!(
        tracked_malloc_headers_matching(&churn_headers),
        churn_headers.len()
    );
    for _ in 0..(GC_MUTATOR_ASSIST_WORK_UNITS * 4) {
        let _ = young_leaf();
    }
    GC_NEXT_MALLOC_TRIGGER.with(|trigger| trigger.set(malloc_object_count().saturating_sub(1)));

    let before = gc_collection_count();
    gc_check_trigger();

    let mut status = budgeted_step_until_phase(GcCyclePhase::AtomicFinalize);
    assert_eq!(status.status, JS_GC_STEP_STATUS_ACTIVE);
    assert_eq!(status.phase, GcCyclePhase::AtomicFinalize.ffi_code());

    let mut assist_steps = 0usize;
    let mut saw_sweep_or_reclaim = false;
    let mut saw_finalize_after_assist = false;
    while js_gc_step_status(&mut status) == JS_GC_STEP_STATUS_ACTIVE {
        let remaining_before = tracked_malloc_headers_matching(&churn_headers);
        gc_check_trigger();
        assist_steps += 1;
        assert!(
            assist_steps < 100_000,
            "allocation-side assists did not drain the late GC phases"
        );
        let current = js_gc_step_status(&mut status);
        if current == JS_GC_STEP_STATUS_IDLE {
            break;
        }
        assert_eq!(current, JS_GC_STEP_STATUS_ACTIVE);
        if status.phase == GcCyclePhase::AtomicFinalize.ffi_code() {
            saw_finalize_after_assist = true;
        }
        if status.phase == GcCyclePhase::Sweep.ffi_code()
            || status.phase == GcCyclePhase::Reclaim.ffi_code()
        {
            saw_sweep_or_reclaim = true;
        }
        let remaining = tracked_malloc_headers_matching(&churn_headers);
        assert!(
            remaining <= remaining_before,
            "allocation-side sweep assist must not resurrect dead malloc entries"
        );
    }
    assert!(
        assist_steps > 0,
        "allocation-side assist should advance late GC phases"
    );
    assert!(
        saw_finalize_after_assist || saw_sweep_or_reclaim,
        "allocation-side assist should report progress through late GC phases"
    );
    assert_eq!(
        js_gc_step_status(&mut status),
        JS_GC_STEP_STATUS_IDLE,
        "allocation-side assists should be able to finish sliced late phases"
    );
    assert!(gc_collection_count() > before);
    assert!(
        saw_sweep_or_reclaim || tracked_malloc_headers_matching(&churn_headers) == 0,
        "allocation-side assists should reach sweep/reclaim work"
    );
    assert!(
        malloc_user_ptr_tracked(live_malloc),
        "live malloc root should survive after allocation assists drain the cycle"
    );
    assert_eq!(
        tracked_malloc_headers_matching(&churn_headers),
        0,
        "allocation-assisted sweep should reclaim dead malloc churn"
    );
}

#[test]
fn allocation_assists_drain_old_reclaim_pressure() {
    let _trace_guard = TestGcTraceCaptureGuard::force_enabled();
    let _guard = CopyingNurseryTestGuard::new(1);
    let _trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();

    let live = live_test_string(b"old_reclaim_assist_live");
    js_shadow_slot_set(0, string_bits(live));

    let dead_old = crate::arena::arena_alloc_gc_old(
        GC_OLD_GEN_RECLAIM_THRESHOLD_BYTES + 1024 * 1024,
        8,
        GC_TYPE_STRING,
    );
    let dead_old_size = unsafe { (*header_from_user_ptr(dead_old as *const u8)).size as u64 };
    let freed_before = GC_STATS.with(|stats| stats.borrow().total_freed_bytes);
    let old_after_alloc = crate::arena::old_gen_in_use_bytes();
    GC_OLD_RECLAIM_PENDING.with(|pending| pending.set(true));
    reset_test_malloc_trim_call_count();

    let before = gc_collection_count();
    gc_check_trigger();

    let mut status = JsGcStepResult::default();
    assert_eq!(js_gc_step_status(&mut status), JS_GC_STEP_STATUS_ACTIVE);
    assert_eq!(status.collection_kind, GcCollectionKind::Full.ffi_code());
    assert_eq!(status.trigger_kind, GcTriggerKind::OldGenBytes.ffi_code());
    assert!(status.old_reclaim_debt_bytes > 0);

    let mut assist_steps = 0usize;
    while js_gc_step_status(&mut status) == JS_GC_STEP_STATUS_ACTIVE {
        gc_check_trigger();
        assist_steps += 1;
        assert!(
            assist_steps < 200_000,
            "allocation-side assists did not drain old reclaim pressure"
        );
    }

    assert!(assist_steps > 0);
    assert!(gc_collection_count() > before);
    assert_eq!(
        test_malloc_trim_call_count(),
        0,
        "ordinary budgeted old reclaim must not invoke process-wide malloc_trim"
    );
    let freed_after = GC_STATS.with(|stats| stats.borrow().total_freed_bytes);
    assert!(
        freed_after.saturating_sub(freed_before) >= dead_old_size,
        "allocation-assisted old reclaim should account freed old-gen bytes"
    );
    assert!(
        crate::arena::old_gen_in_use_bytes() < old_after_alloc,
        "allocation-assisted old reclaim should reduce old-gen in-use bytes"
    );

    let live_after = (js_shadow_slot_get(0) & POINTER_MASK) as *const crate::StringHeader;
    unsafe {
        assert_string_bytes(live_after, b"old_reclaim_assist_live");
    }
    let event = take_test_last_gc_trace_json().expect("old reclaim assist should trace");
    assert_eq!(event["collection_kind"].as_str(), Some("full"));
    assert_eq!(event["trigger"]["kind"].as_str(), Some("old_gen_bytes"));
    assert_eq!(
        event["allocator_maintenance"]["malloc_trim"]["status"].as_str(),
        Some("skipped")
    );
}
