Here is a detailed breakdown of Perry's actual GC surface as it exists in source, followed by a precise mapping of where the architectural divergence from V8/Orinoco occurs.

---

## Perry GC: Current Implementation Surface

### 1. Object Header

Every heap allocation carries an 8-byte `GcHeader` prepended before the user pointer: [1](#0-0)

The `gc_flags` byte packs all generational state into 8 bits: `GC_FLAG_MARKED`, `GC_FLAG_ARENA`, `GC_FLAG_PINNED`, `GC_FLAG_TENURED`, `GC_FLAG_HAS_SURVIVED`, and `GC_FLAG_FORWARDED`. [2](#0-1)

The `_reserved` u16 field doubles as the copied-nursery survival age counter (bits 3–5) and object flags like `OBJ_FLAG_FROZEN`. [3](#0-2)

---

### 2. Arena Regions

The arena is split into multiple named spaces visible in the copying classifier: [4](#0-3)

- **NurseryEden** — bump-allocated from-space
- **Survivor0/Survivor1** — semi-space pair for copying minor GC
- **Longlived** — objects that survive many cycles but aren't old-gen
- **Old** — promoted objects, swept by full mark-sweep
- **Malloc** — tracked separately in `MALLOC_STATE` (promises, maps, large closures)

---

### 3. Two Minor GC Paths

`gc_collect_minor_with_trigger` first attempts a **copying fast path** (`gc_collect_minor_copying_fast_path`), falling back to a **mark-sweep slow path** if the preflight check fails: [5](#0-4)

**Copying fast path** (`CopyingNurseryCollector`): Cheney-style semi-space copy. Live Eden/FromSurvivor objects are moved to ToSurvivor or promoted to Old. Forwarding pointers are installed and all reference sites are rewritten. [6](#0-5)

Promotion is age-based: objects surviving `GC_COPY_PROMOTION_SURVIVALS` cycles are copied to `arena_alloc_gc_old`.

**Mark-sweep slow path**: Used when the copying fast path is ineligible (conservative stack active, barriers inactive, pinned young roots, etc.). Runs a full mark phase then `sweep_with_age_bump`. [7](#0-6)

---

### 4. Root Sources: Precise Shadow Stack + Conservative C-Stack

The shadow stack is a per-thread `Vec<u64>` of NaN-boxed pointer slots. Codegen emits `js_shadow_frame_push`/`js_shadow_slot_set`/`js_shadow_frame_pop` at every function boundary and safepoint: [8](#0-7)

`js_shadow_slot_bind` links a shadow slot to the actual compiled local alloca so the copying GC can rewrite the real slot, not just the mirror: [9](#0-8)

The conservative C-stack scan (`mark_stack_roots_unchecked`) uses `setjmp` for callee-saved registers, then on AArch64 additionally captures all 32 FP registers via inline asm (because LLVM may hold NaN-boxed pointers in caller-saved FP regs across async poll loops): [10](#0-9)

The **auto mode** (`ConservativeStackScanMode::Auto`) skips the conservative scan entirely when a shadow-stack frame is active, falling back to it only when no JS frame is present (i.e., for Rust runtime locals): [11](#0-10)

---

### 5. Write Barrier: Page-Granular Dirty Tracking

`js_write_barrier_slot` fires on every heap store emitted by codegen. It checks old→young generation crossing and marks the 4 KB page containing the written slot as dirty: [12](#0-11)

The remembered set is a `PtrHashSet<usize>` of dirty page indices (`DIRTY_OLD_PAGES`), not a per-object set. Minor GC scans only objects on dirty pages: [13](#0-12)

External malloc-backed slots (e.g., `Map.entries`) use a separate `EXTERNAL_DIRTY_SLOT_PAGES` map keyed by page with owning header addresses: [14](#0-13)

---

### 6. Evacuation Policy (Old-Gen Defrag)

Evacuation of tenured nursery objects to `OLD_ARENA` is **policy-gated**, not unconditional. The policy considers RSS pressure, previous pause time, tenured-bytes-still-in-nursery, and old-page fragmentation: [15](#0-14)

The final decision gate also enforces a pause budget: if the previous pause exceeded 20 ms and RSS is not at hard pressure (256 MB), evacuation is skipped: [16](#0-15)

---

### 7. Trigger Policy

The arena trigger starts at 128 MB with an adaptive step that doubles (≥85% freed) or halves (25–84% freed): [17](#0-16)

A hard ceiling of 128 MB prevents step-doubling from letting peak nursery grow unboundedly: [18](#0-17)

---

### 8. Generational GC is Now Default (Phase D)

`gen_gc_enabled()` returns `true` by default; `PERRY_GEN_GC=0` reverts to full mark-sweep: [19](#0-18)

The plan document confirms Phase D shipped at v0.5.237: [20](#0-19)

---

## Where Perry and V8/Orinoco Diverge

| Dimension | V8 Orinoco | Perry |
|---|---|---|
| **Pause model** | Incremental + concurrent marking; parallel scavenger; mutator runs during most of GC | Stop-the-world for every collection; no concurrent or incremental phases |
| **Minor GC algorithm** | Parallel Scavenger (multiple threads copy simultaneously) | Single-threaded copying (semi-space) or mark-sweep fallback |
| **Major GC algorithm** | Concurrent mark (background threads) + parallel compaction | Single-threaded full mark-sweep; evacuation is policy-gated, not always compacting |
| **Write barrier flavor** | SATB (snapshot-at-the-beginning) for incremental marking; card-table for generational | Page-granular dirty-page log (`DIRTY_OLD_PAGES`); no SATB needed (no concurrent marking) |
| **Root precision** | Fully precise (stack maps generated by TurboFan/Maglev) | Hybrid: precise shadow stack for JS frames + conservative C-stack scan for Rust runtime frames |
| **Conservative scanning** | None (fully precise) | Required for Rust runtime locals; `setjmp` + AArch64 FP register capture |
| **Compaction** | Incremental compaction of old-gen (moves objects, updates all refs) | Evacuation is optional and policy-gated; old-gen is not always compacted |
| **Concurrency** | Background marking threads, concurrent sweeping | No background GC threads; `GC_UNSAFE_ZONES` blocks collection when workers hold JSValue roots |
| **Trigger** | Heap-growth heuristics tuned per isolate | Adaptive arena-bytes + malloc-count triggers with step doubling/halving |

### The Core Tradeoff

Perry's GC is **simpler and correct** but **stop-the-world for every collection**. The pause for a minor GC is bounded by the nursery size (128 MB ceiling) and the number of live objects, but it is a full pause — no mutator progress during collection.

V8's Orinoco eliminates most of that pause by running marking concurrently with the mutator and using incremental fallback. The cost is a much more complex barrier protocol (SATB write barriers to maintain the tri-color invariant during concurrent marking), a fully precise stack-map system (no conservative scanning), and significant engineering complexity in the concurrent/parallel infrastructure.

Perry's design explicitly parks concurrent marking as a future item: [21](#0-20)

The practical consequence: on allocation-heavy workloads (e.g., `bench_json_roundtrip` at 372 ms / 144 MB), Perry's GC pauses are visible and bounded by the 128 MB trigger ceiling. V8 would spread that same work across incremental slices invisible to the application. Perry's advantage is that the entire GC subsystem — including the copying collector, write barrier, shadow stack, and evacuation policy — is implemented in ~3,000 lines of Rust with no background threads, no SATB complexity, and no concurrent data structure hazards.

### Citations

**File:** crates/perry-runtime/src/gc/types.rs (L1-15)
```rust
/// GC header prepended to every heap allocation.
/// Callers receive a pointer AFTER this header (ptr + 8).
#[repr(C)]
pub struct GcHeader {
    /// GC_TYPE_ARRAY, GC_TYPE_STRING, etc.
    pub obj_type: u8,
    /// GC_FLAG_MARKED | GC_FLAG_ARENA | GC_FLAG_PINNED
    pub gc_flags: u8,
    /// Reserved for future use
    pub _reserved: u16,
    /// Total allocation size (header + payload) for arena block walking
    pub size: u32,
}

pub const GC_HEADER_SIZE: usize = std::mem::size_of::<GcHeader>(); // 8 bytes
```

**File:** crates/perry-runtime/src/gc/types.rs (L522-578)
```rust
pub const GC_FLAG_MARKED: u8 = 0x01;
pub const GC_FLAG_ARENA: u8 = 0x02;
pub const GC_FLAG_PINNED: u8 = 0x04;
/// Set on a keys-array that was handed out by `shape_cache_insert`.
/// `js_object_set_field_by_name` reads this bit to decide whether it
/// must clone before mutating (shared arrays can't be mutated in
/// place; fresh arrays allocated in the `keys.is_null()` branch can).
/// Without the bit the clone fires on every property added to every
/// fresh object literal — a 20-property row object allocates 19
/// throwaway keys_array clones per row.
pub const GC_FLAG_SHAPE_SHARED: u8 = 0x08;
/// Set on strings that live in the intern table. Prevents in-place
/// mutation and allows `js_object_set_field_by_name` to skip the
/// FNV-1a hash (pointer identity is sufficient for interned strings).
pub const GC_FLAG_INTERNED: u8 = 0x10;
/// Gen-GC Phase C4: object has survived at least PROMOTION_AGE
/// minor GCs and is now logically tenured — minor GC trace skips
/// recursion into its fields, exactly like an OLD_ARENA-allocated
/// object. Stored on the GcHeader so the per-object check is one
/// byte load + one bit-and. Non-moving generational: tenured
/// objects stay physically in nursery (no copying / forwarding-
/// pointer machinery), but the trace pretends they're old-gen.
/// True compacting evacuation lands in Phase C4b.
pub const GC_FLAG_TENURED: u8 = 0x20;
/// Gen-GC Phase C4: object has survived at least one minor GC.
/// The non-copying minor path still uses this as its one-bit
/// pre-tenure state; the copied-nursery path stores its exact
/// short age in `_reserved` so loop-carried transients get one
/// extra survivor cycle before old-gen promotion.
pub const GC_FLAG_HAS_SURVIVED: u8 = 0x40;
/// Object's user payload begins with a forwarding address. The new
/// address is stored in the **user-payload's first 8 bytes**
/// (immediately after the GcHeader). Walkers that encounter a
/// FORWARDED header read the forwarding address and follow it;
/// ref-rewrite passes update every NaN-boxed pointer they observe to
/// the forwarded address.
///
/// Two runtime paths use the same bit and payload layout:
/// - GC evacuation/copying stubs are short-lived. Evacuation keeps an
///   explicit list of original nursery headers and clears this bit
///   after owned references have been rewritten/verified, so sweep can
///   reclaim the original slot. Copying nursery stubs disappear when
///   from-space is reset.
/// - Array-growth stubs are intentionally retained. `clean_arr_ptr`
///   follows those chains for stale array references that the runtime
///   cannot rewrite.
///
/// Conservative-stack scans STILL get the old (now-stale) address;
/// objects that might be conservatively referenced are pinned out of
/// the evacuation set via `GC_FLAG_PINNED` to avoid corrupting reads
/// from those words.
///
/// This is the last bit in the u8 gc_flags. Adding more flags
/// requires extending GcHeader (currently 8 bytes total — extending
/// breaks ABI everywhere; deferred until/unless a future phase
/// genuinely needs more bits).
pub const GC_FLAG_FORWARDED: u8 = 0x80;
```

**File:** crates/perry-runtime/src/gc/types.rs (L621-630)
```rust
// Object flags stored in GcHeader._reserved (u16) for Object.freeze/seal/preventExtensions
pub const OBJ_FLAG_FROZEN: u16 = 0x01;
pub const OBJ_FLAG_SEALED: u16 = 0x02;
pub const OBJ_FLAG_NO_EXTEND: u16 = 0x04;
// #1175: object was created with a null prototype (Object.create(null) /
// querystring.parse). `Object.getPrototypeOf` returns null for these.
// Bit 6 -- bits 3..5 are the copied-nursery survival counter
// (`GC_COPY_SURVIVAL_AGE_MASK = 0x0038`) and bits 14..15 the layout state,
// so 0x08 would be clobbered on every minor GC. Bits 6..13 are free.
pub const OBJ_FLAG_NULL_PROTO: u16 = 0x40;
```

**File:** crates/perry-runtime/src/gc/copying.rs (L3-11)
```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CopyingPointerKind {
    Eden,
    FromSurvivor,
    ToSurvivor,
    Longlived,
    Old,
    Malloc,
}
```

**File:** crates/perry-runtime/src/gc/copying.rs (L482-550)
```rust
    pub(super) unsafe fn move_young(&mut self, ptr: CopyingPointer) -> usize {
        let header = ptr.header;
        let old_user = (header as *mut u8).add(GC_HEADER_SIZE);
        let flags = (*header).gc_flags;
        if flags & GC_FLAG_FORWARDED != 0 {
            let forwarded = forwarding_address(header) as usize;
            // Array growth also uses GC_FLAG_FORWARDED to leave a stable
            // forwarding stub at the pre-grow address. A root may still point
            // at that stub when copied-minor starts; following it is not
            // enough because the current array can still be in from-space and
            // must itself be marked, moved, and scanned.
            return self.mark_addr(forwarded).unwrap_or(forwarded);
        }

        let total = (*header).size as usize;
        let payload = total - GC_HEADER_SIZE;
        let prior_age = copied_survival_age((*header)._reserved, flags);
        let next_age = prior_age.saturating_add(1);
        let promote = flags & GC_FLAG_TENURED != 0 || next_age >= GC_COPY_PROMOTION_SURVIVALS;
        let new_user = if promote {
            crate::arena::arena_alloc_gc_old(payload, 8, (*header).obj_type)
        } else {
            crate::arena::arena_alloc_gc_survivor(payload, 8, (*header).obj_type)
        };
        std::ptr::copy_nonoverlapping(old_user, new_user, payload);

        let new_header = header_from_user_ptr(new_user);
        (*new_header)._reserved = reserved_with_copied_survival_age(
            (*header)._reserved,
            if promote {
                GC_COPY_PROMOTION_SURVIVALS
            } else {
                next_age
            },
        );
        layout_transfer(old_user, new_user);
        let preserved = flags & (GC_FLAG_SHAPE_SHARED | GC_FLAG_INTERNED | GC_FLAG_PINNED);
        (*new_header).gc_flags = GC_FLAG_ARENA
            | GC_FLAG_MARKED
            | preserved
            | if promote {
                GC_FLAG_TENURED
            } else {
                GC_FLAG_HAS_SURVIVED
            };
        if promote {
            crate::arena::old_page_account_promoted_object(
                new_header as usize,
                total,
                preserved & GC_FLAG_PINNED != 0,
            );
        }

        set_forwarding_address(header, new_user);
        (*header).gc_flags &= !GC_FLAG_MARKED;
        gc_type_after_payload_move((*header).obj_type, old_user as usize, new_user as usize);

        self.worklist.push(new_header);
        self.moved_headers.push(new_header);
        self.live_from_bytes += total;
        if promote {
            self.stats.promoted_objects += 1;
            self.stats.promoted_bytes += total;
        } else {
            self.stats.copied_objects += 1;
            self.stats.copied_bytes += total;
        }
        new_user as usize
    }
```

**File:** crates/perry-runtime/src/gc/mod.rs (L112-138)
```rust
    if let Some(fast_path) = gc_collect_minor_copying_fast_path(&mut trace, start, trigger.kind) {
        let freed_bytes = fast_path.freed_bytes;
        let elapsed_us = start.elapsed().as_micros() as u64;
        GC_STATS.with(|stats| {
            let mut stats = stats.borrow_mut();
            stats.collection_count += 1;
            stats.total_freed_bytes += freed_bytes;
            stats.last_pause_us = elapsed_us;
        });
        GC_FLAGS.with(|f| {
            let cur = f.get();
            if prev_in_alloc != 0 {
                f.set(cur | GC_FLAG_IN_ALLOC);
            } else {
                f.set(cur & !GC_FLAG_IN_ALLOC);
            }
        });
        if let Some(trace) = trace.as_mut() {
            trace.pause_us = elapsed_us;
            trace.capture_layout_scans();
        }
        return GcCollectOutcome {
            freed_bytes,
            malloc_swept: fast_path.malloc_swept,
            trace,
        };
    }
```

**File:** crates/perry-runtime/src/gc/mod.rs (L157-234)
```rust
    // === MARK PHASE (minor) ===
    // Order matters for the C4b pinning policy:
    //
    //   1. Optional conservative C-stack/register scan first. Those
    //      words cannot be rewritten, so when evacuation is enabled
    //      we pin objects discovered by this phase before any
    //      rewriteable root source can add marks. Default `auto`
    //      mode skips this scan while a precise shadow-stack frame is
    //      active; `PERRY_CONSERVATIVE_STACK_SCAN=full` restores the
    //      legacy always-scan fallback.
    //   2. Mutable root slots (shadow stack + registered globals).
    //      These are real slots we can rewrite after forwarding, so
    //      they stay out of CONS_PINNED.
    //   3. Mutable registered scanners. These expose runtime-owned
    //      slots and are revisited by the forwarding rewrite pass, so
    //      they also stay out of CONS_PINNED.
    //   4. Legacy Rust/FFI scanners. Their API exposes copied f64
    //      values only; when evacuation is enabled the scanner
    //      callbacks pin each discovery directly.
    //
    // Pinning only root-direct discoveries keeps heap-field reachability
    // movable: heap fields are handled later by the reference-rewrite
    // pass.
    let phase_start = trace_phase_start(&trace);
    let conservative_scan_decision = conservative_stack_scan_decision();
    let conservative_root_stats =
        mark_stack_roots_for_decision(&valid_ptrs, conservative_scan_decision);
    // CONS_PINNED is only consumed by `evacuate_tenured_nursery_objects`.
    // Stage 1 keeps the low-pressure path from doing the pinning walk.
    let consider_evacuation = evacuation_policy.considered;
    let conservative_pin_stats = if consider_evacuation
        && matches!(
            conservative_scan_decision,
            ConservativeStackScanDecision::Scan
        ) {
        pin_currently_marked_as_conservative()
    } else {
        ConservativePinTraceStats::default()
    };
    match trace.as_mut() {
        Some(trace) => mark_mutable_root_slots(
            &valid_ptrs,
            Some(&mut trace.shadow_roots),
            Some(&mut trace.root_sources),
        ),
        None => mark_mutable_root_slots(&valid_ptrs, None, None),
    }
    match trace.as_mut() {
        Some(trace) => {
            mark_mutable_registered_roots_with_sources(&valid_ptrs, Some(&mut trace.root_sources))
        }
        None => mark_mutable_registered_roots(&valid_ptrs),
    }
    let legacy_root_stats = mark_registered_roots(&valid_ptrs, consider_evacuation);
    if let Some(trace) = trace.as_mut() {
        trace.conservative_root_count = conservative_root_stats.root_count;
        trace.conservative_pinned = conservative_pin_stats.pinned_roots;
        trace.conservative_pinned_bytes = conservative_pin_stats.pinned_bytes;
        trace.legacy_copy_only_scanner_pinned = legacy_root_stats;
        trace.root_sources.native_stack_fallback.decision = conservative_scan_decision;
        trace.root_sources.native_stack_fallback.scanned = matches!(
            conservative_scan_decision,
            ConservativeStackScanDecision::Scan
        );
        trace.root_sources.native_stack_fallback.roots_found = conservative_root_stats.root_count;
        trace.root_sources.native_stack_fallback.pinned_roots = conservative_pin_stats.pinned_roots;
        trace.root_sources.native_stack_fallback.pinned_bytes = conservative_pin_stats.pinned_bytes;
    }
    trace_phase_record(&mut trace, "root_marking", phase_start);
    let phase_start = trace_phase_start(&trace);
    let remembered_set = mark_remembered_set_roots(&valid_ptrs);
    trace_phase_record(&mut trace, "remembered_set_marking", phase_start);
    if let Some(trace) = trace.as_mut() {
        trace.remembered_set = remembered_set;
    }
    let phase_start = trace_phase_start(&trace);
    trace_marked_objects_minor(&valid_ptrs);
    trace_phase_record(&mut trace, "trace_worklist", phase_start);
```

**File:** crates/perry-runtime/src/gc/mod.rs (L431-440)
```rust
pub fn gen_gc_enabled() -> bool {
    use std::sync::OnceLock;
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        !matches!(
            std::env::var("PERRY_GEN_GC").as_deref(),
            Ok("0") | Ok("off") | Ok("false")
        )
    })
}
```

**File:** crates/perry-runtime/src/gc/roots/shadow_stack.rs (L20-47)
```rust
pub(crate) struct ShadowStackState {
    /// `Vec<u64>` instead of `Vec<*mut u8>` because slots hold
    /// NaN-boxed JSValue bits (upper 16 bits are the tag, lower 48
    /// the pointer) — the GC tracer unwraps the NaN-box the same way
    /// it already does for closure captures.
    pub(crate) stack: Vec<u64>,
    /// Optional pointer to the compiled local/global slot represented by
    /// each shadow-stack entry. When present, the GC reads and rewrites the
    /// original slot, not a stale mirror copy.
    pub(crate) slot_ptrs: Vec<usize>,
    /// Liveness bit for each shadow slot. This lets codegen stop reporting a
    /// dead local without mutating the compiled local slot after last use.
    pub(crate) active: Vec<bool>,
    /// Index into `stack` where the current frame's slot_0 lives.
    /// `usize::MAX` when no frame is pushed (initial state + after
    /// the outermost function returns).
    pub(crate) frame_top: usize,
}

thread_local! {
    pub(crate) static SHADOW: std::cell::UnsafeCell<ShadowStackState> =
        std::cell::UnsafeCell::new(ShadowStackState {
            stack: Vec::with_capacity(SHADOW_STACK_GROW_RESERVE),
            slot_ptrs: Vec::with_capacity(SHADOW_STACK_GROW_RESERVE),
            active: Vec::with_capacity(SHADOW_STACK_GROW_RESERVE),
            frame_top: usize::MAX,
        });
}
```

**File:** crates/perry-runtime/src/gc/roots/shadow_stack.rs (L143-160)
```rust
pub extern "C" fn js_shadow_slot_bind(idx: u32, value_slot: *mut u64) {
    if value_slot.is_null() {
        return;
    }
    SHADOW.with(|cell| unsafe {
        let s = &mut *cell.get();
        let top = s.frame_top;
        if top == usize::MAX {
            return;
        }
        let slot = top + idx as usize;
        if slot < s.stack.len() {
            s.slot_ptrs[slot] = value_slot as usize;
            s.stack[slot] = *value_slot;
            s.active[slot] = true;
        }
    });
}
```

**File:** crates/perry-runtime/src/gc/roots.rs (L171-191)
```rust
#[inline]
pub(super) fn conservative_stack_scan_decision_for(
    mode: ConservativeStackScanMode,
    shadow_frame_active: bool,
) -> ConservativeStackScanDecision {
    match mode {
        ConservativeStackScanMode::Disabled => ConservativeStackScanDecision::SkipDisabled,
        ConservativeStackScanMode::Full => ConservativeStackScanDecision::Scan,
        ConservativeStackScanMode::Auto if shadow_frame_active => {
            ConservativeStackScanDecision::SkipShadowStackActive
        }
        ConservativeStackScanMode::Auto => ConservativeStackScanDecision::Scan,
    }
}

pub(super) fn conservative_stack_scan_decision() -> ConservativeStackScanDecision {
    conservative_stack_scan_decision_for(
        conservative_stack_scan_mode(),
        shadow_stack_has_active_frame(),
    )
}
```

**File:** crates/perry-runtime/src/gc/roots.rs (L319-408)
```rust
pub(super) fn mark_stack_roots_unchecked(
    valid_ptrs: &ValidPointerSet,
) -> ConservativeRootTraceStats {
    let mut stats = ConservativeRootTraceStats::default();
    // Capture callee-saved registers into a buffer via setjmp.
    //
    // On Apple platforms the C `setjmp(3)` saves the signal mask via a
    // `sigprocmask` system call, which dominates GC cost (~25 μs per
    // call on arm64). We only need register capture, not signal-state
    // save — switch to `_setjmp(3)` (linker symbol `__setjmp`) on
    // Apple targets. See the matching switch in
    // `promise.rs::js_promise_run_microtasks` for the full rationale.
    //
    // The `setjmp` extern lives in `crate::ffi::setjmp` so this and
    // `promise.rs` share one libc-matching declaration (issue #856).
    // We view the buffer as `u64` slots here because the goal of this
    // path is to scan register-sized words for potential NaN-boxed /
    // raw pointers; the cast to `*mut c_int` at the FFI boundary is
    // the inverse of the cast `promise.rs` does from its `*mut i32`
    // buffer.
    //
    // Size check: 32 * 8 = 256 bytes, which exceeds the darwin arm64
    // `jmp_buf` (48 * 4 = 192 bytes) and every other platform we
    // currently support — see `crate::ffi::setjmp::JMP_BUF_MIN_BYTES`.
    let mut jmp_buf = [0u64; 32]; // oversized for safety
    unsafe {
        crate::ffi::setjmp::setjmp(jmp_buf.as_mut_ptr() as *mut std::os::raw::c_int);
    }

    // Scan the register buffer (covers callee-saved regs: x19-x28 on AArch64, rbx/rbp/r12-r15 on x86_64)
    for &word in &jmp_buf {
        if try_mark_value_or_raw(word, valid_ptrs) {
            stats.root_count += 1;
        }
    }

    // Issue #73: setjmp only captures callee-saved registers. On
    // macOS ARM64 that's x19-x28 + d8-d15 — it misses d0-d7 and
    // d16-d31 (caller-saved FP regs where LLVM may be holding a
    // NaN-boxed pointer across the async poll loop's internal calls,
    // especially under heavy optimization). Capture them explicitly
    // via inline asm so any spilling LLVM hasn't performed is
    // irrelevant — we read the regs directly as they stand at GC
    // entry. A value in d0-d31 ANY of which happens to be a
    // NaN-boxed heap pointer gets marked here.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let mut fp_regs: [u64; 32] = [0; 32];
        std::arch::asm!(
            "str d0,  [{buf}, #0x00]",
            "str d1,  [{buf}, #0x08]",
            "str d2,  [{buf}, #0x10]",
            "str d3,  [{buf}, #0x18]",
            "str d4,  [{buf}, #0x20]",
            "str d5,  [{buf}, #0x28]",
            "str d6,  [{buf}, #0x30]",
            "str d7,  [{buf}, #0x38]",
            "str d8,  [{buf}, #0x40]",
            "str d9,  [{buf}, #0x48]",
            "str d10, [{buf}, #0x50]",
            "str d11, [{buf}, #0x58]",
            "str d12, [{buf}, #0x60]",
            "str d13, [{buf}, #0x68]",
            "str d14, [{buf}, #0x70]",
            "str d15, [{buf}, #0x78]",
            "str d16, [{buf}, #0x80]",
            "str d17, [{buf}, #0x88]",
            "str d18, [{buf}, #0x90]",
            "str d19, [{buf}, #0x98]",
            "str d20, [{buf}, #0xa0]",
            "str d21, [{buf}, #0xa8]",
            "str d22, [{buf}, #0xb0]",
            "str d23, [{buf}, #0xb8]",
            "str d24, [{buf}, #0xc0]",
            "str d25, [{buf}, #0xc8]",
            "str d26, [{buf}, #0xd0]",
            "str d27, [{buf}, #0xd8]",
            "str d28, [{buf}, #0xe0]",
            "str d29, [{buf}, #0xe8]",
            "str d30, [{buf}, #0xf0]",
            "str d31, [{buf}, #0xf8]",
            buf = in(reg) fp_regs.as_mut_ptr(),
            options(nostack, preserves_flags),
        );
        for &word in &fp_regs {
            if try_mark_value_or_raw(word, valid_ptrs) {
                stats.root_count += 1;
            }
        }
    }
```

**File:** crates/perry-runtime/src/gc/barrier.rs (L376-415)
```rust
thread_local! {
    /// Dirty old-generation pages that have received a YOUNG-gen
    /// pointer since the last collection. This is Perry's compact
    /// modbuf: barriers log bounded page regions, and minor GC scans
    /// old objects intersecting those pages.
    pub(crate) static DIRTY_OLD_PAGES: std::cell::RefCell<crate::fast_hash::PtrHashSet<usize>> =
        std::cell::RefCell::new(crate::fast_hash::new_ptr_hash_set());

    /// Dirty non-arena slot pages owned by old-generation parents.
    /// `Map.entries` lives in a malloc buffer behind an old MapHeader,
    /// so its slot page cannot be discovered from the old-arena page
    /// index. Key by external page and retain the owning old headers.
    pub(crate) static EXTERNAL_DIRTY_SLOT_PAGES: std::cell::RefCell<crate::fast_hash::PtrHashMap<usize, Vec<usize>>> =
        std::cell::RefCell::new(crate::fast_hash::new_ptr_hash_map());

    /// Test-only object-level fallback remembered set. Production
    /// barriers use `DIRTY_OLD_PAGES`; tests keep this path available
    /// for parity checks and rollback coverage without a user-facing
    /// runtime mode.
    pub(crate) static REMEMBERED_SET: std::cell::RefCell<std::collections::HashSet<usize>> =
        std::cell::RefCell::new(std::collections::HashSet::new());

    /// Gen-GC Phase C4b: set of GcHeader addresses pinned this
    /// collection cycle because they may be referenced by the
    /// conservative C-stack scan. Conservative scan finds candidate
    /// pointers by bit-pattern matching memory words; we cannot
    /// safely rewrite those words after evacuation because they
    /// might not actually be pointers (false positives). Therefore
    /// any object discovered conservatively is excluded from the
    /// evacuation candidate set.
    ///
    /// Populated by `pin_currently_marked_as_conservative` after
    /// `mark_stack_roots` runs in `gc_collect_minor`. Cleared at
    /// the end of every collection so the next cycle starts fresh.
    pub(crate) static CONS_PINNED: std::cell::RefCell<std::collections::HashSet<usize>> =
        std::cell::RefCell::new(std::collections::HashSet::new());

    pub(super) static WRITE_BARRIER_TRACE_COUNTERS: Cell<BarrierTraceCounters> =
        const { Cell::new(BarrierTraceCounters::zero()) };
}
```

**File:** crates/perry-runtime/src/gc/barrier.rs (L503-573)
```rust
#[no_mangle]
pub extern "C" fn js_write_barrier(parent: u64, child: u64) {
    js_write_barrier_slot(parent, 0, child);
}

/// Gen-GC Phase C1: slot-aware write barrier. Called by
/// codegen-emitted store sites unless `PERRY_WRITE_BARRIERS=0`/
/// `off`/`false` disabled barrier emission at compile time.
///
/// Decode the parent + child as raw addresses. If parent's
/// GcHeader sits in the old-gen arena AND child's NaN-boxed
/// pointer (any of POINTER / STRING / BIGINT / SHORT_STRING)
/// resolves to a heap address inside the nursery, dirty the page
/// containing the written slot. A zero slot address falls back to
/// dirtying every occupied page in the parent object.
///
/// Hot-path constraints: this fires on EVERY heap store in
/// compiled code by default. Must be cheap:
/// generation checks use arena page side metadata rather than
/// scanning every arena block.
#[no_mangle]
pub extern "C" fn js_write_barrier_slot(parent: u64, slot_addr: u64, child: u64) {
    write_barrier_slot_inner(parent, slot_addr as usize, child, false);
}

pub(super) fn write_barrier_slot_inner(
    parent: u64,
    slot_addr: usize,
    child: u64,
    external_slot: bool,
) {
    bump_write_barrier_trace_counter(BarrierTraceCounter::Calls);

    // Decode child first: primitive stores are the most common skip.
    let child_addr = decode_heap_addr(child);
    if child_addr == 0 {
        bump_write_barrier_trace_counter(BarrierTraceCounter::NonPointerChildSkips);
        return;
    }
    // Decode the parent — must be a NaN-boxed heap pointer.
    let parent_addr = decode_heap_addr(parent);
    if parent_addr == 0 {
        bump_write_barrier_trace_counter(BarrierTraceCounter::NonPointerParentSkips);
        return;
    }
    // Old → young check. Runtime-owned malloc GC objects are outside
    // the nursery and must be treated as old when the caller uses the
    // external-slot path for fields or side buffers.
    if !barrier_parent_needs_remembering(parent_addr, external_slot) {
        bump_write_barrier_trace_counter(BarrierTraceCounter::ParentNotOldSkips);
        return;
    }
    if !matches!(
        crate::arena::classify_heap_generation(child_addr),
        crate::arena::HeapGeneration::Nursery
    ) {
        bump_write_barrier_trace_counter(BarrierTraceCounter::ChildNotYoungSkips);
        return;
    }

    bump_write_barrier_trace_counter(BarrierTraceCounter::OldToYoungSlowHits);
    bump_write_barrier_trace_counter(BarrierTraceCounter::RememberedSetInsertAttempts);
    let inserted = if external_slot {
        remember_old_to_young_external_slot(parent_addr, slot_addr)
    } else {
        remember_old_to_young_slot(parent_addr, slot_addr)
    };
    if inserted {
        bump_write_barrier_trace_counter(BarrierTraceCounter::NewInserts);
    }
}
```

**File:** crates/perry-runtime/src/gc/oldgen.rs (L183-265)
```rust
pub(super) fn evacuation_policy_initial_decision(
    tenured_still_in_nursery_bytes: usize,
    rss_bytes: u64,
    previous_pause_us: u64,
    pre_evac_pause_us: u64,
    allowed: bool,
    force: bool,
    old_to_young_tracking_complete: bool,
    old_page_selected_pages: usize,
) -> EvacuationPolicyDecision {
    let snapshot = EvacuationPolicySnapshot {
        tenured_still_in_nursery_bytes,
        rss_bytes,
        previous_pause_us,
        pre_evac_pause_us,
        ..EvacuationPolicySnapshot::default()
    };
    if !allowed {
        return EvacuationPolicyDecision {
            allowed,
            force,
            reason: "disabled",
            snapshot,
            ..EvacuationPolicyDecision::default()
        };
    }
    if !old_to_young_tracking_complete {
        return EvacuationPolicyDecision {
            allowed,
            force,
            reason: "barriers_inactive",
            snapshot,
            ..EvacuationPolicyDecision::default()
        };
    }
    if force {
        return EvacuationPolicyDecision {
            allowed,
            considered: true,
            force,
            reason: "force_considered",
            snapshot,
            ..EvacuationPolicyDecision::default()
        };
    }
    if tenured_still_in_nursery_bytes >= MIN_TENURED_NURSERY_BYTES {
        return EvacuationPolicyDecision {
            allowed,
            considered: true,
            force,
            reason: "nursery_pressure",
            snapshot,
            ..EvacuationPolicyDecision::default()
        };
    }
    if rss_bytes >= RSS_PRESSURE_BYTES {
        return EvacuationPolicyDecision {
            allowed,
            considered: true,
            force,
            reason: "rss_pressure",
            snapshot,
            ..EvacuationPolicyDecision::default()
        };
    }
    if old_page_selected_pages > 0 {
        return EvacuationPolicyDecision {
            allowed,
            considered: true,
            force,
            reason: "old_page_fragmentation",
            snapshot,
            ..EvacuationPolicyDecision::default()
        };
    }
    EvacuationPolicyDecision {
        allowed,
        force,
        reason: "low_pressure",
        snapshot,
        ..EvacuationPolicyDecision::default()
    }
}
```

**File:** crates/perry-runtime/src/gc/oldgen.rs (L402-421)
```rust
    let hard_rss_pressure = snapshot.rss_bytes >= RSS_HARD_PRESSURE_BYTES;
    let pause_budget_exceeded = snapshot.previous_pause_us > MAX_PREVIOUS_PAUSE_US
        || snapshot.pre_evac_pause_us > MAX_PREVIOUS_PAUSE_US;
    if pause_budget_exceeded && !hard_rss_pressure {
        decision.reason = "pause_budget_exceeded";
        return decision;
    }
    decision.enabled = true;
    decision.reason = if hard_rss_pressure {
        "rss_hard_pressure"
    } else if snapshot.rss_bytes >= RSS_PRESSURE_BYTES {
        "rss_pressure"
    } else if snapshot.old_page_selected_pages > 0
        && snapshot.tenured_still_in_nursery_bytes < MIN_TENURED_NURSERY_BYTES
    {
        "old_page_fragmentation"
    } else {
        "nursery_pressure"
    };
    decision
```

**File:** crates/perry-runtime/src/gc/oldgen.rs (L473-488)
```rust
/// Generational GC (minor collection on every trigger) is now the
/// default model as of Phase D (v0.5.237). Set `PERRY_GEN_GC=0`,
/// `=false`, or `=off` to opt out and fall back to the full
/// mark-sweep — kept as an escape hatch for bisecting GC-related
/// regressions in user programs.
///
/// Why generational is the default now: Phase C (v0.5.222-228) wired
/// the nursery / old-gen split, write barriers, remembered set, and
/// non-moving tenuring; Phase C4b (v0.5.229-236) added forwarding
/// pointer infrastructure, conservative-pinning safety, policy-gated
/// evacuation, reference rewriting,
/// idle-block deallocation, and the trigger ceiling that bounds
/// peak nursery occupancy. The minor-GC path has been the proven-
/// equivalent default in every regression suite (168 unit tests,
/// 9 `test_json_*.ts` × 4 mode combos, 18 memory-stability tests)
/// since C3b landed; flipping the default makes those gains apply
```

**File:** crates/perry-runtime/src/gc/policy.rs (L22-69)
```rust
pub(super) const GC_THRESHOLD_INITIAL_BYTES: usize = 128 * 1024 * 1024; // 128 MB
/// Sanity bound on the adaptive step itself. Step growth past 1 GB is
/// only theoretically possible on multi-day services where GC fires
/// rarely; we keep the cap loose here since the *real* peak-RSS
/// guardrail is `GC_TRIGGER_ABSOLUTE_CEILING` below.
pub(super) const GC_THRESHOLD_MAX_BYTES: usize = 1024 * 1024 * 1024; // 1 GB

/// Hard ceiling on the next-GC trigger (arena_total bytes), independent
/// of how productive recent sweeps have been. Without this, the
/// >90%-freed branch doubles the step on every productive collection,
/// > and `next_trigger = new_total + step` lets peak nursery occupancy
/// > grow unboundedly even when most of what we collected was garbage.
/// > On `bench_json_roundtrip` direct (50 iters × ~5 MB / iter, GC fires
/// > 3 times), the step doubled from 64 MB → 67 MB → 134 MB and the
/// > trigger followed it, so peak nursery hit 115 MB at GC #3 — the
/// > dealloc pass from C4b-δ then returned 91 MB to the OS, but the
/// > peak-RSS damage was already done. Capping the trigger at the
/// > initial threshold prevents that runaway: after GC, trigger ≤ 128 MB
/// > regardless of how much step adapted, so peak nursery stays bounded
/// > to roughly initial + one iter's allocation buffer + headroom for
/// > non-arena overhead.
///
/// Floor: even if `arena_total` is already near or past the ceiling
/// (large old-gen + longlived combined live set), keep at least the
/// 16 MB step floor as headroom — `next_trigger = max(new_total + 16 MB,
/// min(new_total + step, ceiling))`. This avoids GC thrash when the
/// non-nursery component of arena_total alone exceeds the ceiling.
///
/// 2026-05-02 raise from 64 MB → 128 MB: ECS perf-comprehensive's
/// allocation-heavy benches (10k two-comp + sync, 5k × 3 cmds) hit
/// the 64 MB cap mid-round, then the >25%-freed branch halved the
/// step to 16 MB, so the next trigger landed ~16 MB above the post-
/// GC working set — well within a single round's allocation budget.
/// Result: 1-2 mid-round GCs per bench, the worst of which spent
/// 60 ms inside `mark_block_persisting_arena_objects` force-marking
/// + tracing 40 k newly-allocated objects in the recent window.
/// Doubling the cap lets productive sweeps accumulate full
/// `step` headroom (up to 128 MB) before the next trigger, which
/// shifts those GC events out of the measured rounds entirely.
/// `bench_json_roundtrip`-class workloads still bounded — they
/// finish under 128 MB peak and fire ≤2 GCs total.
///
/// Workloads unaffected: `07_object_create` / `12_binary_trees` /
/// `bench_gc_pressure` all fit their working sets under 64 MB and
/// fire GC at most once. The cap only changes behavior when the step
/// would otherwise have pushed the trigger past the initial threshold,
/// which is exactly the bench-RSS scenario this is targeting.
pub(super) const GC_TRIGGER_ABSOLUTE_CEILING: usize = 128 * 1024 * 1024;
```

**File:** crates/perry-runtime/src/gc/policy.rs (L791-802)
```rust
        let new_total = arena_total_bytes();
        // C4b-δ-tune: hard cap on next_trigger so the >90%-freed
        // step-doubling can't drive peak nursery past the initial
        // threshold. Floor: at least 16 MB of headroom past
        // `new_total` so a workload whose post-GC live set already
        // approaches the ceiling doesn't thrash on every fresh
        // allocation.
        let stepped = new_total.saturating_add(step);
        let capped = stepped.min(GC_TRIGGER_ABSOLUTE_CEILING);
        let floor = new_total.saturating_add(16 * 1024 * 1024);
        let next_trigger = std::cmp::max(capped, floor);
        GC_NEXT_TRIGGER_BYTES.with(|c| c.set(next_trigger));
```

**File:** docs/generational-gc-plan.md (L486-488)
```markdown
- **Concurrent marking:** old-gen major GC could run concurrently
  with mutator. Multi-week effort; park until generational itself
  is stable.
```
