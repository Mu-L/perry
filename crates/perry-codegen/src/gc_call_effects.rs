//! Perry-GC call effects for native-stack safepoint lowering.
//!
//! This is deliberately narrower than LLVM's memory-effect attributes. A
//! helper may mutate runtime metadata, take a lock, or allocate through the
//! system allocator and still be safe to omit as a Perry GC safepoint. The
//! only question answered here is: can this call enter Perry's collector?
//!
//! Unknown is the safe default. Adding a helper to the allowlist requires
//! auditing the complete runtime call graph for `gc_check_trigger`,
//! `js_gc_collect`, `js_gc_loop_safepoint`, or another route into collection.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GcCallEffect {
    CannotCollect,
    /// May allocate — and therefore arm a GC trigger — but never runs a
    /// collection synchronously inside the call and never re-enters generated
    /// JS (no getters, setters, valueOf/toString coercion, or callbacks).
    ///
    /// Only meaningful under `PERRY_GC_SAFEPOINT_ONLY`: the runtime contract
    /// guarantees any trigger these helpers arm either defers to a declared
    /// safepoint (moving) or collects behind a forced conservative scan (the
    /// alloc-point valve), so the caller's precise frame roots are never
    /// consumed at this call site and it needs no statepoint. Without the
    /// contract these remain safepoints.
    AllocNoReentry,
    Unknown,
}

/// Classify one direct LLVM callee name, without the leading `@`.
pub(crate) fn classify_direct_callee(name: &str) -> GcCallEffect {
    match name {
        // Pure/read-only ABI helpers audited in `module::helper_decl_attrs`.
        "js_nanbox_pointer"
        | "js_nanbox_get_pointer"
        | "js_typed_f64_arg_guard"
        | "js_typed_i32_arg_guard"
        | "js_typed_i1_arg_guard"
        | "js_typed_i1_arg_to_raw"
        | "js_typed_i32_arg_to_raw"
        | "js_typed_string_arg_guard"
        | "js_is_truthy"
        | "js_typed_feedback_plain_array_index_get_guard"
        | "js_typed_feedback_numeric_array_index_get_guard"
        | "js_typed_feedback_plain_array_index_set_guard"
        | "js_typed_feedback_numeric_array_index_set_guard"
        | "js_typed_feedback_numeric_array_push_guard"
        | "js_array_numeric_value_to_raw_f64"
        // `gc/roots/temp_roots.rs`: TLS vector operations and an incremental
        // marking barrier only. They never run a Perry collection.
        | "js_gc_temp_root_push"
        | "js_gc_temp_root_get"
        | "js_gc_temp_root_set"
        | "js_gc_temp_root_truncate"
        // `gc/barrier.rs`: remembered-set / incremental-marking maintenance.
        | "js_write_barrier"
        | "js_write_barrier_slot"
        | "js_write_barrier_root_heap_word"
        | "js_write_barrier_root_nanbox"
        // `gc/layout.rs`: side-table metadata updates only.
        | "js_gc_note_slot_layout"
        | "js_gc_note_slot_layout_aware"
        | "js_gc_init_typed_shape_layout"
        | "js_gc_init_unboxed_object_layout"
        // `typed_feedback.rs`: counters/registries only. This intentionally
        // does not include feedback wrappers that perform the actual object
        // get/set operation.
        | "js_typed_feedback_record_guard_pass"
        | "js_typed_feedback_record_guard_fail"
        | "js_typed_feedback_record_fallback_call"
        | "js_typed_feedback_class_field_get_guard"
        | "js_typed_feedback_class_field_set_guard"
        | "js_typed_feedback_observe_property_get"
        | "js_typed_feedback_observe_property_set"
        // Refcount writes and array-layout observations; none enters GC.
        | "js_string_addref"
        | "js_string_addref_if_heap_string"
        | "js_array_clear_numeric_layout"
        | "js_array_note_numeric_write"
        | "js_array_is_numeric_f64_layout"
        // TLS dynamic-call context only.
        | "js_implicit_this_set"
        | "js_new_target_get"
        | "js_new_target_set" => GcCallEffect::CannotCollect,
        // Audited allocate-but-never-reenter helpers (2026-07-31): each body
        // was checked for closure invocation, coercion (valueOf/toString),
        // and accessor dispatch — none present, and none takes a receiver
        // that could route through user code (`js_array_length` takes a
        // typed `*const ArrayHeader`, not a JSValue). The forced-evacuation
        // probe gates backstop the audit.
        "js_closure_alloc_singleton"
        | "js_object_alloc_class_inline_keys"
        | "js_array_push_f64"
        | "js_array_length"
        | "js_array_slice_values" => GcCallEffect::AllocNoReentry,
        _ => GcCallEffect::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audited_runtime_bookkeeping_cannot_collect() {
        for name in [
            "js_gc_temp_root_push",
            "js_write_barrier_root_nanbox",
            "js_gc_note_slot_layout",
            "js_typed_feedback_record_guard_pass",
            "js_string_addref",
        ] {
            assert_eq!(
                classify_direct_callee(name),
                GcCallEffect::CannotCollect,
                "{name}"
            );
        }
    }

    #[test]
    fn collection_and_unknown_calls_stay_conservative() {
        for name in [
            "js_gc_collect",
            "js_gc_loop_safepoint",
            "js_alloc_object",
            "user_function",
        ] {
            assert_eq!(
                classify_direct_callee(name),
                GcCallEffect::Unknown,
                "{name}"
            );
        }
    }

    #[test]
    fn audited_alloc_helpers_are_contract_only_non_safepoints() {
        for name in ["js_closure_alloc_singleton", "js_array_push_f64"] {
            assert_eq!(
                classify_direct_callee(name),
                GcCallEffect::AllocNoReentry,
                "{name}"
            );
        }
        // Re-entering helpers must never be in the AllocNoReentry class:
        // a poll can fire inside the callback/getter with this frame
        // mid-stack, and the caller's roots must be findable.
        for name in [
            "js_array_map",
            "js_array_sort_with_comparator",
            "js_number_coerce",
            "js_dynamic_string_or_number_add",
            "js_object_get_field_by_name_f64",
        ] {
            assert_eq!(
                classify_direct_callee(name),
                GcCallEffect::Unknown,
                "{name}"
            );
        }
    }
}
