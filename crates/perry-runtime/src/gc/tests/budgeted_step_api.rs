use super::super::*;
use super::support::*;

fn reset_old_reclaim_pressure() {
    let old_in_use = crate::arena::old_gen_in_use_bytes();
    GC_LAST_OLD_RECLAIM_IN_USE_BYTES.with(|bytes| bytes.set(old_in_use));
    GC_OLD_RECLAIM_PENDING.with(|pending| pending.set(false));
}

fn synchronous_only_test_root_scanner(_visitor: &mut RuntimeRootVisitor<'_>) {}

#[test]
fn no_pressure_budgeted_step_reports_idle_without_starting_cycle() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let _trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();

    let before = gc_collection_count();
    let mut result = JsGcStepResult::default();

    assert_eq!(js_gc_step_status(&mut result), JS_GC_STEP_STATUS_IDLE);
    assert_eq!(result.active, 0);
    assert_eq!(result.completed, 0);

    assert_eq!(
        js_gc_step_work_units(0, &mut result),
        JS_GC_STEP_STATUS_IDLE
    );
    assert_eq!(js_gc_step_us(0, &mut result), JS_GC_STEP_STATUS_IDLE);
    assert_eq!(
        js_gc_step_work_units(1, &mut result),
        JS_GC_STEP_STATUS_IDLE
    );
    assert_eq!(gc_collection_count(), before);
}

#[test]
fn arena_pressure_budgeted_step_starts_bounded_minor_cycle() {
    let _guard = CopyingNurseryTestGuard::new(2);
    let _barrier_guard = GeneratedWriteBarrierTestGuard::active();
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();

    let live = young_leaf();
    js_shadow_slot_set(0, ptr_bits(live));
    let _dead = young_leaf();
    trigger_guard.make_arena_trigger_due();

    let mut result = JsGcStepResult::default();
    assert_eq!(
        js_gc_step_work_units(1, &mut result),
        JS_GC_STEP_STATUS_ACTIVE
    );
    assert_eq!(result.active, 1);
    assert_eq!(result.completed, 0);
    assert_eq!(result.collection_kind, GcCollectionKind::Minor.ffi_code());
    assert_eq!(result.trigger_kind, GcTriggerKind::ArenaBytes.ffi_code());
    assert_eq!(result.phase, GcCyclePhase::BuildValidPointerSet.ffi_code());
    assert!(result.arena_debt_bytes > 0);

    assert_eq!(js_gc_step_status(&mut result), JS_GC_STEP_STATUS_ACTIVE);
    assert_eq!(result.active, 1);

    let completed = complete_budgeted_gc_cycle();
    assert_eq!(completed.status, JS_GC_STEP_STATUS_COMPLETED);
    assert_eq!(completed.active, 0);
    assert_eq!(completed.completed, 1);

    assert_eq!(js_gc_step_status(&mut result), JS_GC_STEP_STATUS_IDLE);
    assert_eq!(result.active, 0);
    assert_ne!(
        (js_shadow_slot_get(0) & POINTER_MASK) as usize,
        live,
        "eligible budgeted arena pressure should use copied-minor and rewrite the root"
    );
}

#[test]
fn arena_pressure_budgeted_step_reports_copied_minor_eligibility() {
    let _trace_guard = TestGcTraceCaptureGuard::force_enabled();
    let _guard = CopyingNurseryTestGuard::new(1);
    let _barrier_guard = GeneratedWriteBarrierTestGuard::active();
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();

    let live = young_leaf();
    js_shadow_slot_set(0, ptr_bits(live));
    let _dead = young_leaf();
    trigger_guard.make_arena_trigger_due();

    let mut result = JsGcStepResult::default();
    assert_eq!(
        js_gc_step_work_units(1, &mut result),
        JS_GC_STEP_STATUS_ACTIVE
    );
    assert_eq!(result.collection_kind, GcCollectionKind::Minor.ffi_code());
    assert_eq!(result.trigger_kind, GcTriggerKind::ArenaBytes.ffi_code());

    let completed = complete_budgeted_gc_cycle();
    assert_eq!(completed.status, JS_GC_STEP_STATUS_COMPLETED);
    let after = (js_shadow_slot_get(0) & POINTER_MASK) as usize;
    assert_ne!(after, live);
    assert!(crate::arena::pointer_in_nursery(after));

    let event = take_test_last_gc_trace_json().expect("copied-minor budgeted GC should trace");
    assert_eq!(event["collection_kind"].as_str(), Some("minor"));
    assert_eq!(event["trigger"]["kind"].as_str(), Some("arena_bytes"));
    assert_eq!(event["copying_nursery"]["eligible"].as_bool(), Some(true));
    assert_eq!(
        event["copying_nursery"]["fallback_reason"].as_str(),
        Some("none")
    );
    assert!(
        event["copying_nursery"]["copied_objects"]
            .as_u64()
            .unwrap_or(0)
            > 0,
        "eligible budgeted arena pressure should copy live nursery objects"
    );
}

#[test]
fn arena_pressure_budgeted_step_reports_copied_minor_fallback_reason() {
    let _trace_guard = TestGcTraceCaptureGuard::force_enabled();
    let _guard = CopyingNurseryTestGuard::new(1);
    let _barrier_guard = GeneratedWriteBarrierTestGuard::inactive();
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();

    let live = young_leaf();
    js_shadow_slot_set(0, ptr_bits(live));
    let _dead = young_leaf();
    trigger_guard.make_arena_trigger_due();

    let mut result = JsGcStepResult::default();
    assert_eq!(
        js_gc_step_work_units(1, &mut result),
        JS_GC_STEP_STATUS_ACTIVE
    );
    assert_eq!(result.collection_kind, GcCollectionKind::Minor.ffi_code());
    assert_eq!(result.trigger_kind, GcTriggerKind::ArenaBytes.ffi_code());

    let completed = complete_budgeted_gc_cycle();
    assert_eq!(completed.status, JS_GC_STEP_STATUS_COMPLETED);
    assert_eq!(js_shadow_slot_get(0) & POINTER_MASK, live as u64);

    let event = take_test_last_gc_trace_json().expect("fallback budgeted GC should trace");
    assert_eq!(event["collection_kind"].as_str(), Some("minor"));
    assert_eq!(event["trigger"]["kind"].as_str(), Some("arena_bytes"));
    assert_eq!(event["copying_nursery"]["eligible"].as_bool(), Some(false));
    assert_eq!(
        event["copying_nursery"]["fallback_reason"].as_str(),
        Some("barriers_inactive")
    );
}

fn assert_automatic_copied_minor_failure_hands_off_survivor_promotion(
    trigger_kind: GcTriggerKind,
    register_blocking_scanner: bool,
) {
    let payload = LARGE_OBJECT_THRESHOLD_BYTES - GC_HEADER_SIZE - 64;
    let object_count = (GC_COPY_PROMOTION_HANDOFF_MIN_BYTES / payload) + 64;
    let _trace_guard = TestGcTraceCaptureGuard::force_enabled();
    let _guard = CopyingNurseryTestGuard::new(object_count as u32);
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();

    let mut originals = Vec::with_capacity(object_count);
    for slot in 0..object_count {
        let child = crate::arena::arena_alloc_gc(payload, 8, GC_TYPE_STRING) as usize;
        assert!(crate::arena::pointer_in_nursery(child));
        js_shadow_slot_set(slot as u32, ptr_bits(child));
        originals.push(child);
    }

    let first_trace = collect_minor_trace(GcTriggerKind::Direct);
    assert_copied_minor_trace(&first_trace, true, CopiedMinorFallbackReason::None, false);

    let mut survivor_total = 0usize;
    for (slot, original) in originals.iter().enumerate() {
        let survivor = (js_shadow_slot_get(slot as u32) & POINTER_MASK) as usize;
        assert_ne!(survivor, *original);
        assert!(crate::arena::pointer_in_nursery(survivor));
        unsafe {
            let header = header_from_user_ptr(survivor as *const u8);
            survivor_total = survivor_total.saturating_add((*header).size as usize);
            (*header).gc_flags |= GC_FLAG_TENURED;
        }
    }
    assert!(survivor_total >= GC_COPY_PROMOTION_HANDOFF_MIN_BYTES);

    reset_old_reclaim_pressure();
    if register_blocking_scanner {
        gc_register_mutable_root_scanner(synchronous_only_test_root_scanner);
    }
    let _old_pressure =
        crate::arena::arena_alloc_gc_old(GC_OLD_GEN_RECLAIM_THRESHOLD_BYTES, 8, GC_TYPE_OBJECT);
    assert!(copied_minor_promotion_handoff_due(trigger_kind));

    let _barrier_guard = GeneratedWriteBarrierTestGuard::inactive();
    match trigger_kind {
        GcTriggerKind::ArenaBytes => trigger_guard.make_arena_trigger_due(),
        GcTriggerKind::MallocCount => trigger_guard.make_malloc_sweep_due(),
        other => panic!(
            "unsupported automatic copied-minor trigger: {}",
            other.as_str()
        ),
    }

    let mut result = JsGcStepResult::default();
    assert_eq!(
        js_gc_step_work_units(1, &mut result),
        JS_GC_STEP_STATUS_ACTIVE
    );
    assert_eq!(result.collection_kind, GcCollectionKind::Minor.ffi_code());
    assert_eq!(result.trigger_kind, trigger_kind.ffi_code());

    assert_eq!(
        js_gc_step_work_units(1, &mut result),
        JS_GC_STEP_STATUS_ACTIVE
    );
    assert_eq!(result.collection_kind, GcCollectionKind::Full.ffi_code());
    assert_eq!(
        result.trigger_kind,
        GcTriggerKind::SurvivorPromotionBytes.ffi_code()
    );

    let completed = complete_budgeted_gc_cycle();
    assert_eq!(completed.status, JS_GC_STEP_STATUS_COMPLETED);
    assert_eq!(completed.collection_kind, GcCollectionKind::Full.ffi_code());
    assert_eq!(
        completed.trigger_kind,
        GcTriggerKind::SurvivorPromotionBytes.ffi_code()
    );

    let event = take_test_last_gc_trace_json().expect("survivor-promotion handoff should trace");
    assert_eq!(event["collection_kind"].as_str(), Some("full"));
    assert_eq!(
        event["trigger"]["kind"].as_str(),
        Some("survivor_promotion_bytes")
    );
}

#[test]
fn arena_pressure_copied_minor_failure_hands_off_survivor_promotion_full() {
    assert_automatic_copied_minor_failure_hands_off_survivor_promotion(
        GcTriggerKind::ArenaBytes,
        false,
    );
}

#[test]
fn malloc_pressure_copied_minor_failure_hands_off_survivor_promotion_full() {
    assert_automatic_copied_minor_failure_hands_off_survivor_promotion(
        GcTriggerKind::MallocCount,
        false,
    );
}

#[test]
fn scanner_blocked_copied_minor_failure_hands_off_survivor_promotion_full() {
    assert_automatic_copied_minor_failure_hands_off_survivor_promotion(
        GcTriggerKind::ArenaBytes,
        true,
    );
}

#[test]
fn gc_init_default_scanners_do_not_block_automatic_budgeted_trigger() {
    let _trace_guard = TestGcTraceCaptureGuard::force_enabled();
    let _guard = CopyingNurseryTestGuard::new(1);
    let _barrier_guard = GeneratedWriteBarrierTestGuard::active();
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();
    gc_init();

    let live = young_leaf();
    js_shadow_slot_set(0, ptr_bits(live));
    let _dead = young_leaf();
    trigger_guard.make_arena_trigger_due();

    let before = gc_collection_count();
    gc_check_trigger();

    let mut status = JsGcStepResult::default();
    assert_eq!(
        js_gc_step_status(&mut status),
        JS_GC_STEP_STATUS_IDLE,
        "eligible copied-minor pressure should finish during allocation-side assist"
    );
    assert!(gc_collection_count() > before);
    let after = (js_shadow_slot_get(0) & POINTER_MASK) as usize;
    assert_ne!(after, live);

    let event = take_test_last_gc_trace_json().expect("automatic copied-minor GC should trace");
    assert_eq!(event["collection_kind"].as_str(), Some("minor"));
    assert_eq!(event["trigger"]["kind"].as_str(), Some("arena_bytes"));
    assert_eq!(event["copying_nursery"]["eligible"].as_bool(), Some(true));
    assert_eq!(
        event["copying_nursery"]["fallback_reason"].as_str(),
        Some("none")
    );
}

#[test]
fn synchronous_only_registered_scanner_allows_eligible_copied_minor() {
    let _trace_guard = TestGcTraceCaptureGuard::force_enabled();
    let _guard = CopyingNurseryTestGuard::new(1);
    let _barrier_guard = GeneratedWriteBarrierTestGuard::active();
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();
    gc_register_mutable_root_scanner(synchronous_only_test_root_scanner);

    let live = young_leaf();
    js_shadow_slot_set(0, ptr_bits(live));
    let _dead = young_leaf();
    trigger_guard.make_arena_trigger_due();

    let before = gc_collection_count();
    gc_check_trigger();

    let mut status = JsGcStepResult::default();
    assert_eq!(
        js_gc_step_status(&mut status),
        JS_GC_STEP_STATUS_IDLE,
        "eligible copied-minor should finish before synchronous scanner blocks fallback"
    );
    assert!(gc_collection_count() > before);
    let after = (js_shadow_slot_get(0) & POINTER_MASK) as usize;
    assert_ne!(after, live);

    let event = take_test_last_gc_trace_json().expect("automatic copied-minor GC should trace");
    assert_eq!(event["collection_kind"].as_str(), Some("minor"));
    assert_eq!(event["trigger"]["kind"].as_str(), Some("arena_bytes"));
    assert_eq!(event["copying_nursery"]["eligible"].as_bool(), Some(true));
    assert_eq!(
        event["copying_nursery"]["fallback_reason"].as_str(),
        Some("none")
    );
    assert_eq!(
        event["root_sources"]["runtime_mutable_scanners"]["registered_scanners"].as_u64(),
        Some(1)
    );
}

#[test]
fn copied_minor_pressure_runs_before_scanner_blocked_old_reclaim() {
    let _trace_guard = TestGcTraceCaptureGuard::force_enabled();
    let _guard = CopyingNurseryTestGuard::new(1);
    let _barrier_guard = GeneratedWriteBarrierTestGuard::active();
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();
    gc_register_mutable_root_scanner(synchronous_only_test_root_scanner);

    let live = young_leaf();
    js_shadow_slot_set(0, ptr_bits(live));
    let _dead = young_leaf();
    GC_OLD_RECLAIM_PENDING.with(|pending| pending.set(true));
    trigger_guard.make_arena_trigger_due();

    let before = gc_collection_count();
    gc_check_trigger();

    let mut result = JsGcStepResult::default();
    assert_eq!(
        js_gc_step_status(&mut result),
        JS_GC_STEP_STATUS_IDLE,
        "eligible copied-minor should not be hidden behind scanner-blocked old reclaim"
    );
    assert!(gc_collection_count() > before);
    let moved = (js_shadow_slot_get(0) & POINTER_MASK) as usize;
    assert_ne!(moved, live);

    let event = take_test_last_gc_trace_json().expect("prioritized copied-minor should trace");
    assert_eq!(event["collection_kind"].as_str(), Some("minor"));
    assert_eq!(event["trigger"]["kind"].as_str(), Some("arena_bytes"));
    assert_eq!(event["copying_nursery"]["eligible"].as_bool(), Some(true));
    assert_eq!(
        event["copying_nursery"]["fallback_reason"].as_str(),
        Some("none")
    );

    GC_OLD_RECLAIM_PENDING.with(|pending| pending.set(false));
}

#[test]
fn synchronous_only_registered_scanner_blocks_copied_minor_fallback() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();
    gc_register_mutable_root_scanner(synchronous_only_test_root_scanner);

    {
        let _barrier_guard = GeneratedWriteBarrierTestGuard::inactive();
        let live = young_leaf();
        js_shadow_slot_set(0, ptr_bits(live));
        trigger_guard.make_arena_trigger_due();

        let mut result = JsGcStepResult::default();
        assert_eq!(
            js_gc_step_work_units(1, &mut result),
            JS_GC_STEP_STATUS_ACTIVE
        );
        assert_eq!(result.active, 1);
        assert_eq!(result.collection_kind, GcCollectionKind::Minor.ffi_code());
        assert_eq!(result.phase, GcCyclePhase::BuildValidPointerSet.ffi_code());

        assert_eq!(
            js_gc_step_work_units(1, &mut result),
            JS_GC_STEP_STATUS_SKIPPED
        );
        assert_eq!(result.active, 0);
        assert_eq!(result.completed, 0);
        assert_eq!(js_gc_step_status(&mut result), JS_GC_STEP_STATUS_IDLE);
    }

    let _barrier_guard = GeneratedWriteBarrierTestGuard::active();
    let live_after_skip = young_leaf();
    js_shadow_slot_set(0, ptr_bits(live_after_skip));
    let _dead_after_skip = young_leaf();
    trigger_guard.make_arena_trigger_due();

    let before = gc_collection_count();
    gc_check_trigger();

    let mut result = JsGcStepResult::default();
    assert_eq!(js_gc_step_status(&mut result), JS_GC_STEP_STATUS_IDLE);
    assert!(
        gc_collection_count() > before,
        "eligible copied-minor should still run after fallback was scanner-blocked"
    );
    let moved_after_skip = (js_shadow_slot_get(0) & POINTER_MASK) as usize;
    assert_ne!(moved_after_skip, live_after_skip);
}

#[test]
fn repeated_budgeted_steps_complete_full_cycle_and_reclaim_unreachable_objects() {
    let _guard = CopyingNurseryTestGuard::new(2);
    let _trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    reset_old_reclaim_pressure();

    let live_child = young_leaf();
    let live_malloc = gc_malloc(
        std::mem::size_of::<crate::closure::ClosureHeader>() + std::mem::size_of::<u64>(),
        GC_TYPE_CLOSURE,
    );
    unsafe {
        init_test_closure_with_one_capture(live_malloc, ptr_bits(live_child));
    }
    js_shadow_slot_set(0, ptr_bits(live_malloc as usize));

    let dead_malloc_headers = allocate_dead_malloc_churn_headers(8);
    let dead_old = crate::arena::arena_alloc_gc_old(32, 8, GC_TYPE_STRING);
    let dead_old_size = unsafe { (*header_from_user_ptr(dead_old as *const u8)).size as u64 };
    let freed_before = GC_STATS.with(|stats| stats.borrow().total_freed_bytes);

    GC_OLD_RECLAIM_PENDING.with(|pending| pending.set(true));
    let mut result = JsGcStepResult::default();
    assert_eq!(
        js_gc_step_work_units(1, &mut result),
        JS_GC_STEP_STATUS_ACTIVE
    );
    assert_eq!(result.collection_kind, GcCollectionKind::Full.ffi_code());
    assert_eq!(result.trigger_kind, GcTriggerKind::OldGenBytes.ffi_code());

    let completed = complete_budgeted_gc_cycle();
    assert_eq!(completed.status, JS_GC_STEP_STATUS_COMPLETED);
    assert_eq!(
        completed.phase,
        GcCyclePhase::Complete.ffi_code(),
        "final step should report completed phase"
    );

    assert!(
        malloc_user_ptr_tracked(live_malloc),
        "live malloc root should remain tracked"
    );
    assert_eq!(
        tracked_malloc_headers_matching(&dead_malloc_headers),
        0,
        "unreachable malloc churn should be swept"
    );
    let freed_after = GC_STATS.with(|stats| stats.borrow().total_freed_bytes);
    assert!(
        freed_after.saturating_sub(freed_before) >= dead_old_size,
        "full budgeted sweep should reclaim unreachable old-arena bytes"
    );

    assert_eq!(js_gc_step_status(&mut result), JS_GC_STEP_STATUS_IDLE);
}

#[test]
fn microsecond_budget_step_remains_bounded_on_multi_slice_heap() {
    let _guard = CopyingNurseryTestGuard::new(1);
    let trigger_guard = GcTriggerThresholdTestGuard::suppress_automatic_triggers();
    let _barrier_guard = GeneratedWriteBarrierTestGuard::inactive();
    reset_old_reclaim_pressure();

    let live = young_leaf();
    js_shadow_slot_set(0, ptr_bits(live));
    for _ in 0..5_000 {
        let _ = young_leaf();
    }
    trigger_guard.make_arena_trigger_due();

    let before = gc_collection_count();
    let mut result = JsGcStepResult::default();
    assert_eq!(js_gc_step_us(1, &mut result), JS_GC_STEP_STATUS_ACTIVE);
    assert_eq!(result.active, 1);
    assert_eq!(result.completed, 0);
    assert_eq!(gc_collection_count(), before);

    let completed = complete_budgeted_gc_cycle();
    assert_eq!(completed.status, JS_GC_STEP_STATUS_COMPLETED);
    assert_eq!(js_shadow_slot_get(0) & POINTER_MASK, live as u64);
}
