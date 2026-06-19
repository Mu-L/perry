use super::super::*;

fn assert_additive_pause_telemetry_fields(
    event: &serde_json::Value,
    expected_kind: &str,
    expected_class: &str,
    ordinary: bool,
) {
    let pause_budget = &event["pause_budget"];
    assert_eq!(pause_budget["kind"].as_str(), Some(expected_kind));
    assert_eq!(pause_budget["class"].as_str(), Some(expected_class));
    assert_eq!(pause_budget["budget_unit"].as_str(), Some("work_units"));
    assert_eq!(pause_budget["ordinary_budgeted"].as_bool(), Some(ordinary));
    assert_eq!(
        pause_budget["ordinary_pause_stats_include"].as_bool(),
        Some(ordinary)
    );
    assert!(pause_budget["max_observed_step_pause_us"]
        .as_u64()
        .is_some());
    assert!(event["pause_steps"].as_array().is_some());
    assert!(event["phase_progression"]
        .as_array()
        .expect("phase_progression should be an array")
        .iter()
        .any(|phase| phase.as_str() == Some("build_valid_pointer_set")));
    for key in ["start", "end", "max_observed"] {
        assert!(event["debt"][key]["arena_debt_bytes"].as_u64().is_some());
        assert!(event["debt"][key]["malloc_debt_objects"].as_u64().is_some());
        assert!(event["debt"][key]["old_reclaim_debt_bytes"]
            .as_u64()
            .is_some());
    }
}

#[test]
fn test_gc_progress_contract_defaults() {
    let contract = gc_progress_contract();

    assert_eq!(contract.mode, GcRuntimeMode::Throughput);
    assert_eq!(
        contract.normal_step_budget,
        GcPauseBudget::bounded(
            GC_NORMAL_INCREMENTAL_WORK_UNITS,
            GC_NORMAL_INCREMENTAL_SOFT_PAUSE_US,
        )
    );
    assert_eq!(
        contract.assist_budget,
        GcPauseBudget::bounded(
            GC_MUTATOR_ASSIST_WORK_UNITS,
            GC_MUTATOR_ASSIST_SOFT_PAUSE_US,
        )
    );
    assert!(contract.normal_step_budget.is_bounded());
    assert!(contract.assist_budget.is_bounded());
    assert_eq!(
        contract.explicit_synchronous_policy,
        GcPauseBudget::unbounded()
    );
    assert_eq!(contract.explicit_full_policy, GcPauseBudget::unbounded());
    assert_eq!(contract.emergency_policy, GcPauseBudget::unbounded());
    assert_eq!(
        GcProgressKind::ExplicitSynchronous.as_str(),
        "explicit_synchronous"
    );
    assert_eq!(GcProgressKind::ExplicitFull.as_str(), "explicit_full");
    assert_eq!(GcProgressKind::EmergencyFull.as_str(), "emergency_full");
}

#[test]
fn test_gc_progress_contract_latency_mode_tightens_budget() {
    let _mode = GcModeTestGuard::set(GcRuntimeMode::Latency);
    let contract = gc_progress_contract();

    assert_eq!(contract.mode, GcRuntimeMode::Latency);
    assert_eq!(
        contract.normal_step_budget,
        GcPauseBudget::bounded(
            GC_LATENCY_INCREMENTAL_WORK_UNITS,
            GC_LATENCY_INCREMENTAL_SOFT_PAUSE_US,
        )
    );
    assert_eq!(
        contract.assist_budget,
        GcPauseBudget::bounded(
            GC_LATENCY_MUTATOR_ASSIST_WORK_UNITS,
            GC_LATENCY_MUTATOR_ASSIST_SOFT_PAUSE_US,
        )
    );
    assert!(
        contract.normal_step_budget.work_units.unwrap() < GC_NORMAL_INCREMENTAL_WORK_UNITS,
        "latency mode should spend smaller host-driven slices"
    );
    assert!(
        contract.assist_budget.work_units.unwrap() < GC_MUTATOR_ASSIST_WORK_UNITS,
        "latency mode should spend smaller allocation-side assists"
    );
    assert_eq!(
        gc_mutator_assist_work_units(),
        GC_LATENCY_MUTATOR_ASSIST_WORK_UNITS,
        "gc_check_trigger must consume the mode-specific mutator-assist budget"
    );
}

#[test]
fn test_gc_latency_mode_effective_trigger_uses_young_debt_ceiling() {
    let _mode = GcModeTestGuard::set(GcRuntimeMode::Latency);
    let previous = GC_NEXT_TRIGGER_BYTES.with(|trigger| {
        let previous = trigger.get();
        trigger.set(GC_THRESHOLD_INITIAL_BYTES);
        previous
    });

    assert_eq!(
        gc_effective_next_arena_trigger(),
        GC_LATENCY_TRIGGER_ABSOLUTE_CEILING
    );
    assert_eq!(
        GcDebtSnapshot::current().arena_debt_bytes,
        gc_nursery_trigger_bytes().saturating_sub(GC_LATENCY_TRIGGER_ABSOLUTE_CEILING) as u64
    );

    GC_NEXT_TRIGGER_BYTES.with(|trigger| trigger.set(previous));
}

#[test]
fn test_gc_progress_kind_is_budgeted_only_for_incremental_and_assist() {
    assert!(GcProgressKind::NormalIncremental.is_budgeted());
    assert!(GcProgressKind::MutatorAssist.is_budgeted());
    assert!(!GcProgressKind::ExplicitSynchronous.is_budgeted());
    assert!(!GcProgressKind::ExplicitFull.is_budgeted());
    assert!(!GcProgressKind::EmergencyFull.is_budgeted());
    assert!(!GcProgressKind::LegacySynchronous.is_budgeted());
}

#[test]
fn test_gc_progress_contract_trace_json_labels_automatic_as_legacy() {
    let trace = GcCycleTrace::new(
        GcCollectionKind::Minor,
        GcTriggerSnapshot {
            kind: GcTriggerKind::ArenaBytes,
            steps_before: Some(GcStepSnapshot::current()),
        },
    )
    .expect("test requested GC trace capture");

    let event = trace.into_json(GcStepSnapshot::current());
    let progress = &event["progress_contract"];

    assert_eq!(progress["mode"].as_str(), Some("throughput"));
    assert_eq!(progress["kind"].as_str(), Some("legacy_synchronous"));
    assert_eq!(progress["budget_unit"].as_str(), Some("work_units"));
    assert!(progress["configured_work_budget"].is_null());
    assert!(progress["soft_pause_target_us"].is_null());
    assert_eq!(progress["ordinary_budgeted"].as_bool(), Some(false));
    assert_eq!(progress["class"].as_str(), Some("legacy"));
    assert_additive_pause_telemetry_fields(&event, "legacy_synchronous", "legacy", false);
}

#[test]
fn test_gc_progress_contract_trace_json_records_latency_mode() {
    let _mode = GcModeTestGuard::set(GcRuntimeMode::Latency);
    let mut trace = GcCycleTrace::new(
        GcCollectionKind::Minor,
        GcTriggerSnapshot {
            kind: GcTriggerKind::ArenaBytes,
            steps_before: Some(GcStepSnapshot::current()),
        },
    )
    .expect("test requested GC trace capture");
    trace.progress_kind = GcProgressKind::NormalIncremental;

    let event = trace.into_json(GcStepSnapshot::current());
    let progress = &event["progress_contract"];

    assert_eq!(progress["mode"].as_str(), Some("latency"));
    assert_eq!(progress["kind"].as_str(), Some("normal_incremental"));
    assert_eq!(
        progress["configured_work_budget"].as_u64(),
        Some(GC_LATENCY_INCREMENTAL_WORK_UNITS as u64)
    );
    assert_eq!(
        progress["soft_pause_target_us"].as_u64(),
        Some(GC_LATENCY_INCREMENTAL_SOFT_PAUSE_US)
    );
    assert_eq!(
        event["pause_budget"]["kind"].as_str(),
        Some("normal_incremental")
    );
}

#[test]
fn test_gc_progress_contract_trace_json_labels_manual_minor_as_explicit_sync() {
    let trace = GcCycleTrace::new(
        GcCollectionKind::Minor,
        GcTriggerSnapshot {
            kind: GcTriggerKind::Manual,
            steps_before: Some(GcStepSnapshot::current()),
        },
    )
    .expect("test requested GC trace capture");

    let event = trace.into_json(GcStepSnapshot::current());
    let progress = &event["progress_contract"];

    assert_eq!(event["collection_kind"].as_str(), Some("minor"));
    assert_eq!(event["trigger"]["kind"].as_str(), Some("manual"));
    assert_eq!(progress["kind"].as_str(), Some("explicit_synchronous"));
    assert_eq!(progress["budget_unit"].as_str(), Some("work_units"));
    assert!(progress["configured_work_budget"].is_null());
    assert!(progress["soft_pause_target_us"].is_null());
    assert_eq!(progress["ordinary_budgeted"].as_bool(), Some(false));
    assert_eq!(progress["class"].as_str(), Some("explicit"));
    assert_additive_pause_telemetry_fields(&event, "explicit_synchronous", "explicit", false);
}
