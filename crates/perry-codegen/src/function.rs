//! LLVM IR function builder.
//!
//! Port of `anvil/src/llvm/function.ts`. A function owns a `RegCounter` shared
//! by all its blocks (see `block.rs`), an ordered list of blocks, and emits
//! itself as an LLVM `define` when serialized.

use std::rc::Rc;

use crate::block::{FpFlags, LlBlock, RegCounter};
use crate::types::LlvmType;

pub struct LlFunction {
    pub name: String,
    pub return_type: LlvmType,
    pub params: Vec<(LlvmType, String)>,
    /// Optional LLVM linkage string, e.g. `"internal"` or `"private"`. Empty
    /// string means external (default) linkage.
    pub linkage: String,
    /// When true, the function body contains a `try` statement (setjmp/longjmp),
    /// so the definition gets `#1` (`noinline`) and `to_ir` runs the volatile
    /// promotion pass.
    ///
    /// The setjmp hazard: `longjmp` restores the callee-saved registers and
    /// stack pointer that `setjmp` snapshotted, so any local LLVM parked in a
    /// register across the setjmp call reverts to its setjmp-time value when
    /// the exception fires — the try body's mutations vanish in the catch.
    /// `returns_twice` on the setjmp call is not sufficient at -O2 on aarch64.
    ///
    /// This used to be handled by stamping `optnone` on the whole function,
    /// which is correct (at -O0 every value is frame-resident) but cost ~5x on
    /// the surrounding code even when nothing ever throws (#6385). We now apply
    /// C's `volatile` rule precisely instead: only the allocas the try body
    /// actually stores into get volatile accesses, and everything else in the
    /// function stays optimizable. See [`crate::volatile_setjmp`].
    pub has_try: bool,
    /// When true, emit `alwaysinline` attribute. Forces LLVM to inline this
    /// function at every call site, exposing integer operations to the
    /// caller's optimizer context (critical for vectorization of clamp patterns).
    pub force_inline: bool,
    /// When true (and `force_inline` is not), emit the `inlinehint` attribute.
    /// Unlike `alwaysinline`, `inlinehint` only *raises* LLVM's inline
    /// threshold for this callee — LLVM keeps its `-O3` growth budget and can
    /// still decline to inline into cold / many call sites. Set for small
    /// functions with a hot (in-loop) call site so a bit-mixer-style kernel
    /// gets inlined into its loop without the binary-size blowup an
    /// unconditional `alwaysinline` threshold bump causes. See the
    /// inline-hot-small heuristic in `codegen/function.rs`. `alwaysinline`
    /// already implies the hint, so the two are never emitted together, and
    /// `has_try` (noinline) still wins over both in `to_ir`.
    pub inline_hint: bool,
    blocks: Vec<LlBlock>,
    block_counter: u32,
    reg_counter: Rc<RegCounter>,
    fp_flags: FpFlags,
    /// Allocas hoisted to the function entry block. These are emitted at
    /// the very top of block 0 at IR-serialization time, so they dominate
    /// every use everywhere in the function.
    ///
    /// LLVM convention is that all `alloca` instructions live in the
    /// function entry block — that way the slot pointer is in scope from
    /// every reachable basic block. Putting an alloca inside an `if` arm
    /// works only when its uses are also in that arm; the moment a closure
    /// captures the slot from a sibling branch (or any code reached after
    /// the if-merge), we get "Instruction does not dominate all uses" from
    /// the LLVM verifier.
    ///
    /// Use `LlFunction::alloca_entry(ty)` to allocate; the helper bumps
    /// the shared register counter so the returned `%r<N>` name is unique
    /// function-wide, then appends `"  %r<N> = alloca <ty>"` to this list.
    /// `to_ir()` prepends the list to entry-block instructions in order.
    entry_allocas: Vec<String>,
    /// Hoisted setup instructions (loads, stores, calls) that must run
    /// AFTER the entry block's "init prelude" — `js_gc_init` and the
    /// `__perry_init_strings_*` calls — but BEFORE any user code, so
    /// they dominate every reachable use yet see the up-to-date module
    /// state. Used by the inline-allocator hoist for per-class
    /// `keys_array` global loads: the global is populated by
    /// `__perry_init_strings_*`, so loading it at the very top of the
    /// entry block (in `entry_allocas`) reads zero. Splicing the load
    /// in just after the init calls fixes that without losing the
    /// loop-invariant hoisting benefit on the hot allocation path.
    ///
    /// `to_ir()` splices these instructions into block 0 at the
    /// `entry_init_boundary` instruction index. If no boundary is set
    /// (e.g. user functions, which have no init prelude), they are
    /// emitted immediately after entry allocas and before the first
    /// block instruction so the dominance guarantee still holds.
    entry_post_init_setup: Vec<String>,
    /// Index in block 0's instruction list where `entry_post_init_setup`
    /// should be spliced in. Set by `mark_entry_init_boundary` after
    /// the init prelude has been emitted; left as `None` for functions
    /// with no init prelude.
    entry_init_boundary: Option<usize>,
    /// Shadow-stack frame slot (gen-GC Phase A sub-phase 2). When
    /// `Some(slot_reg)`, `to_ir()` rewrites every `ret` in the
    /// function body to call `js_shadow_frame_pop` first, reading
    /// the frame handle stored at `slot_reg`. The push is emitted by
    /// either `enable_shadow_frame` (top of entry) or
    /// `enable_post_init_shadow_frame` (after the entry init prelude).
    ///
    /// `None` means no shadow frame — `ret` instructions pass
    /// through unchanged. Currently gated per-function so we can
    /// land wiring incrementally (e.g. just `main`) before
    /// flipping the default across every user function.
    shadow_frame_slot: Option<String>,
    /// Entry alloca holding this thread's `ShadowStackState` address, so the
    /// inline slot stores (#7088) can address the buffer without a per-store
    /// thread-local lookup. Set alongside `shadow_frame_slot`.
    shadow_state_slot: Option<String>,
    /// Whether shadow-frame emission was requested for this function at all
    /// (i.e. `enable_shadow_frame` / `enable_post_init_shadow_frame` ran).
    ///
    /// Distinct from `shadow_frame_slot.is_some()`: a function whose locals
    /// were all proven non-pointer requests a frame but gets none, because a
    /// zero-slot frame is pure overhead. `reserve_shadow_slot` needs to tell
    /// that case (grow it — there is now something to root) apart from "the
    /// shadow stack is switched off for this build" (do nothing).
    shadow_frame_requested: bool,
    /// Which region the frame push belongs in — `entry_post_init_setup` when
    /// `enable_post_init_shadow_frame` was used, `entry_allocas` otherwise.
    /// Remembered so a lazily-created frame lands where the eager one would.
    shadow_frame_post_init_region: bool,
    /// Where the emitted `js_shadow_frame_push` line lives, so its slot count
    /// can be rewritten when lowering discovers a root the pre-lowering
    /// pointer analysis could not see (#6968: scalar-replaced object fields
    /// and array elements, which have no HIR local of their own).
    shadow_frame_push: Option<ShadowFramePush>,
    /// Slot count currently baked into that push line.
    shadow_frame_slot_count: u32,
    /// Research backend: preserve the existing precise-root slot numbering,
    /// but encode the slots in LLVM stack maps instead of allocating a
    /// parallel runtime shadow frame.
    stack_map_requested: bool,
    /// Logical root slots reserved by the existing liveness analysis. The
    /// final IR pass resolves these indices to the native allocas named by
    /// `js_shadow_slot_bind` calls, removes the calls, and emits stack maps.
    stack_map_slot_count: u32,
    /// Runtime hooks emitted immediately before each non-pointer `ret`.
    /// Entry/module-init functions use this for process-level diagnostics
    /// that must run regardless of which block reaches the normal epilogue.
    pre_return_void_calls: Vec<String>,
}

/// Render the frame-push instruction. Kept in one place so the eager
/// emission and the later count rewrite cannot drift.
///
/// `js_shadow_frame_enter` is `js_shadow_frame_push` returning the address of
/// this thread's `ShadowStackState` instead of the frame handle, so the inline
/// slot stores (#7088) get their base pointer without a second thread-local
/// lookup. The handle the matching pop needs is recovered from the state by
/// [`shadow_frame_handle_lines`] — `handle == frame_top - HEADER_SLOTS` — so
/// the pop side is untouched.
fn shadow_frame_push_line(state_reg: &str, slot_count: u32) -> String {
    format!(
        "  {} = call ptr @js_shadow_frame_enter(i32 {})",
        state_reg, slot_count
    )
}

/// The lines following the push: stash the state pointer for the inline slot
/// stores, then recover the frame handle from `ShadowStackState::frame_top`.
///
/// Offsets mirror `perry_runtime::gc::roots::SHADOW_STATE_FRAME_TOP_OFFSET`
/// and `SHADOW_STACK_HEADER_SLOTS`; `perry`'s `shadow_layout_contract` test
/// pins them to the runtime's.
fn shadow_frame_handle_lines(
    state_reg: &str,
    state_slot: &str,
    handle_slot: &str,
    top_ptr_reg: &str,
    top_reg: &str,
    handle_reg: &str,
) -> Vec<String> {
    use crate::expr::shadow_inline::{SHADOW_STACK_HEADER_SLOTS, SHADOW_STATE_FRAME_TOP_OFFSET};
    vec![
        format!("  store ptr {}, ptr {}", state_reg, state_slot),
        format!(
            "  {} = getelementptr inbounds i8, ptr {}, i64 {}",
            top_ptr_reg, state_reg, SHADOW_STATE_FRAME_TOP_OFFSET
        ),
        format!("  {} = load i64, ptr {}", top_reg, top_ptr_reg),
        format!(
            "  {} = sub i64 {}, {}",
            handle_reg, top_reg, SHADOW_STACK_HEADER_SLOTS
        ),
        format!("  store i64 {}, ptr {}", handle_reg, handle_slot),
    ]
}

/// Location of a function's `js_shadow_frame_push` line, so its slot-count
/// operand can be rewritten in place after the fact.
struct ShadowFramePush {
    /// `true` when the line lives in `entry_post_init_setup` rather than
    /// `entry_allocas`.
    post_init: bool,
    /// Index of the line within that region.
    line_idx: usize,
    /// SSA register the push result is assigned to, needed to re-render.
    handle_reg: String,
}

impl LlFunction {
    pub fn new(
        name: impl Into<String>,
        return_type: LlvmType,
        params: Vec<(LlvmType, String)>,
    ) -> Self {
        Self::new_with_fp_flags(name, return_type, params, FpFlags::default())
    }

    pub fn new_with_fp_flags(
        name: impl Into<String>,
        return_type: LlvmType,
        params: Vec<(LlvmType, String)>,
        fp_flags: FpFlags,
    ) -> Self {
        Self {
            name: name.into(),
            return_type,
            params,
            linkage: String::new(),
            has_try: false,
            force_inline: false,
            inline_hint: false,
            blocks: Vec::new(),
            block_counter: 0,
            reg_counter: Rc::new(RegCounter::new()),
            fp_flags,
            entry_allocas: Vec::new(),
            entry_post_init_setup: Vec::new(),
            entry_init_boundary: None,
            shadow_frame_slot: None,
            shadow_state_slot: None,
            shadow_frame_requested: false,
            shadow_frame_post_init_region: false,
            shadow_frame_push: None,
            shadow_frame_slot_count: 0,
            stack_map_requested: false,
            stack_map_slot_count: 0,
            pre_return_void_calls: Vec::new(),
        }
    }

    /// Enable shadow-stack frame emission for this function (gen-GC
    /// Phase A sub-phase 2). Emits `js_shadow_frame_push(slot_count)`
    /// into `entry_allocas` so it runs at the top of block 0, stores
    /// the returned u64 handle into a fresh alloca, and records the
    /// slot for the `to_ir()` ret-rewriting pass to load from.
    ///
    /// Safe to call at most once per function. After this call,
    /// `to_ir()` will insert a matching
    /// `js_shadow_frame_pop(loaded_handle)` before every `ret` in
    /// the function body, regardless of which codegen path emitted
    /// the ret. Frame balance is preserved automatically.
    ///
    /// Passing `slot_count = 0` is a no-op: the frame would only carry
    /// a (prev_top, slot_count) header with no GC-root slots — that is
    /// pure overhead, an extra TLS-touching call per function entry +
    /// per ret. Today every leaf function with no pointer-typed locals
    /// (clampIdx, clampU8, imul32, …) hits this case, and when the
    /// function is `alwaysinline` the push/pop pair gets duplicated
    /// into every caller's hot loop. Skip the frame entirely; the
    /// to_ir() rewrite pass keys off `shadow_frame_slot.is_some()`,
    /// so no matching pop is emitted either.
    pub fn enable_shadow_frame(&mut self, slot_count: u32) {
        self.enable_shadow_frame_inner(slot_count, false);
    }

    /// Enable shadow-stack frame emission for entry/module-init functions
    /// whose first block contains runtime init prelude calls. The handle slot
    /// still lives in `entry_allocas` so it dominates all returns, but the
    /// `js_shadow_frame_push` call runs through `entry_post_init_setup`, after
    /// `mark_entry_init_boundary()` has marked `js_gc_init` / string-init
    /// completion and before any top-level user code is lowered.
    pub fn enable_post_init_shadow_frame(&mut self, slot_count: u32) {
        self.enable_shadow_frame_inner(slot_count, true);
    }

    fn enable_shadow_frame_inner(&mut self, slot_count: u32, post_init: bool) {
        if crate::codegen::helpers::native_stack_roots_enabled() {
            self.shadow_frame_requested = true;
            self.shadow_frame_post_init_region = post_init;
            self.stack_map_requested = slot_count != 0;
            self.stack_map_slot_count = slot_count;
            return;
        }
        if self.shadow_frame_slot.is_some() {
            return;
        }
        // Record the request (and its region) even when no frame is emitted:
        // `reserve_shadow_slot` uses it to tell "nothing to root yet" from
        // "shadow stack disabled", and to place a lazily-created push line in
        // the same region `enable_*_shadow_frame` would have used.
        self.shadow_frame_requested = true;
        self.shadow_frame_post_init_region = post_init;
        if slot_count == 0 {
            return;
        }
        self.emit_shadow_frame_push(slot_count, post_init);
    }

    fn emit_shadow_frame_push(&mut self, slot_count: u32, post_init: bool) {
        use crate::types::{I64, PTR};
        let handle_slot = self.alloca_entry(I64);
        let state_slot = self.alloca_entry(PTR);
        // Null-initialize in `entry_allocas`, which is always spliced at the
        // very top of block 0 — so the slot is initialized even when the push
        // itself lives in `entry_post_init_setup` (spliced later, after the
        // runtime init prelude). An inline slot store that somehow ran before
        // the push would then read null and take its runtime-call arm rather
        // than an undef pointer. Where the push dominates, LLVM sees the later
        // store of a `nonnull` return and folds that arm away.
        self.entry_allocas
            .push(format!("  store ptr null, ptr {}", state_slot));
        let state_reg = format!("%r{}", self.reg_counter.next());
        let top_ptr_reg = format!("%r{}", self.reg_counter.next());
        let top_reg = format!("%r{}", self.reg_counter.next());
        let handle_reg = format!("%r{}", self.reg_counter.next());
        let push_line = shadow_frame_push_line(&state_reg, slot_count);
        let rest = shadow_frame_handle_lines(
            &state_reg,
            &state_slot,
            &handle_slot,
            &top_ptr_reg,
            &top_reg,
            &handle_reg,
        );
        let region = if post_init {
            &mut self.entry_post_init_setup
        } else {
            &mut self.entry_allocas
        };
        let line_idx = region.len();
        region.push(push_line);
        region.extend(rest);
        self.shadow_frame_push = Some(ShadowFramePush {
            post_init,
            line_idx,
            handle_reg: state_reg,
        });
        self.shadow_frame_slot_count = slot_count;
        self.shadow_frame_slot = Some(handle_slot);
        self.shadow_state_slot = Some(state_slot);
    }

    /// The entry alloca holding this thread's `ShadowStackState` address, when
    /// this function pushed a shadow frame. `None` means the inline slot
    /// stores have no base to work from and callers must use the `extern "C"`
    /// entry points.
    pub fn shadow_state_slot(&self) -> Option<&str> {
        self.shadow_state_slot.as_deref()
    }

    /// Reserve one more GC-root slot in this function's shadow frame and
    /// return its index, rewriting the already-emitted
    /// `js_shadow_frame_push` count in place.
    ///
    /// `collect_pointer_typed_locals` sizes the frame before lowering, from
    /// the HIR locals it can see. Scalar replacement (#6968) creates storage
    /// that has no HIR local of its own — one entry-block alloca per object
    /// field / array element — so a heap value living in one of those is
    /// invisible to the pre-lowering count. Rather than teach the collector
    /// to predict every scalar-replacement decision (they are taken later, on
    /// conditions the collector does not evaluate), the frame grows on demand
    /// at the store site that actually needs the root.
    ///
    /// Returns `None` when shadow-stack emission is switched off for this
    /// build, in which case the caller must not emit slot traffic either.
    /// When the frame was skipped as empty, one is created here.
    pub fn reserve_shadow_slot(&mut self) -> Option<u32> {
        if !self.shadow_frame_requested {
            return None;
        }
        if crate::codegen::helpers::native_stack_roots_enabled() {
            let idx = self.stack_map_slot_count;
            self.stack_map_slot_count += 1;
            self.stack_map_requested = true;
            return Some(idx);
        }
        if self.shadow_frame_push.is_none() {
            let post_init = self.shadow_frame_post_init_region;
            self.emit_shadow_frame_push(0, post_init);
        }
        let idx = self.shadow_frame_slot_count;
        self.shadow_frame_slot_count += 1;
        let count = self.shadow_frame_slot_count;
        let Some(push) = &self.shadow_frame_push else {
            return None;
        };
        let (post_init, line_idx) = (push.post_init, push.line_idx);
        let line = shadow_frame_push_line(&push.handle_reg, count);
        let region = if post_init {
            &mut self.entry_post_init_setup
        } else {
            &mut self.entry_allocas
        };
        region[line_idx] = line;
        Some(idx)
    }

    /// Mark the current end of the entry block as the boundary between
    /// the init prelude (`js_gc_init`, `__perry_init_strings_*`) and
    /// user code. Hoisted post-init setup (cached global loads) is
    /// spliced in at this point so it dominates every use yet sees the
    /// initialized module state. Call this once, immediately after the
    /// codegen has emitted the init prelude into block 0 and before any
    /// user statement is lowered.
    pub fn mark_entry_init_boundary(&mut self) {
        if let Some(blk) = self.blocks.first() {
            self.entry_init_boundary = Some(blk.instruction_count());
        } else {
            self.entry_init_boundary = Some(0);
        }
    }

    pub fn add_pre_return_void_call(&mut self, func_name: impl Into<String>) {
        self.pre_return_void_calls.push(func_name.into());
    }

    /// Open a setjmp-protected region (#6385). Every `store` emitted into any
    /// block of this function until the matching [`exit_try_region`] is
    /// recorded as "modified between the setjmp and a possible longjmp", and
    /// the alloca behind it is given volatile accesses by `to_ir`.
    ///
    /// Call this around the lowering of a `try` body and of a `catch` body
    /// that a `finally` re-protects — i.e. exactly the code that a `longjmp`
    /// can cut short. Regions nest; the depth counter handles that.
    ///
    /// [`exit_try_region`]: LlFunction::exit_try_region
    pub fn enter_try_region(&self) {
        self.reg_counter.enter_try_region();
    }

    pub fn exit_try_region(&self) {
        self.reg_counter.exit_try_region();
    }

    /// Allocate a fresh stack slot in the function entry block. Returns
    /// the SSA pointer name (e.g. `%r42`). The instruction is emitted at
    /// the top of block 0, ahead of any existing entry-block code, so
    /// the slot dominates every reachable use — even from inside nested
    /// if/else branches that would otherwise produce a "does not dominate
    /// all uses" verifier error.
    pub fn alloca_entry(&mut self, ty: LlvmType) -> String {
        let r = format!("%r{}", self.reg_counter.next());
        self.entry_allocas.push(format!("  {} = alloca {}", r, ty));
        r
    }

    /// Allocate a fixed-size `[count x elem_ty]` array slot in the function
    /// entry block. Returned register is a `ptr` to the array; index it with
    /// `gep(elem_ty, reg, [(I64, i)])`.
    ///
    /// LLVM lowers a non-entry-block `alloca` as a runtime `sub %rsp, N`
    /// with no matching restore — every loop iteration through such a block
    /// permanently shrinks the stack. Issue #167 hit this for the args-array
    /// allocas in `js_native_call_method` dispatch sites: a tight
    /// `for (i = 0; i < N; i++) buf.readInt32BE(i*4)` ate ~16 bytes of stack
    /// per iteration and SIGSEGV'd around iteration 250k–300k. The cure is
    /// to hoist these allocas to the entry block (executed once at function
    /// prologue) — what this helper enforces.
    pub fn alloca_entry_array(&mut self, elem_ty: LlvmType, count: usize) -> String {
        let r = format!("%r{}", self.reg_counter.next());
        self.entry_allocas
            .push(format!("  {} = alloca [{} x {}]", r, count, elem_ty));
        r
    }

    /// Allocate a byte buffer in the entry block with an explicit ABI
    /// alignment. Used for C-layout POD records where field GEPs must rest on
    /// a verifier-checked stack object, not JS object storage.
    pub fn alloca_entry_bytes_aligned(&mut self, size: u32, alignment: u32) -> String {
        let r = format!("%r{}", self.reg_counter.next());
        self.entry_allocas.push(format!(
            "  {} = alloca [{} x i8], align {}",
            r, size, alignment
        ));
        r
    }

    /// Push a store instruction into the entry-block alloca section.
    /// Used to initialize allocas to a safe default (e.g. TAG_UNDEFINED)
    /// at the top of the function, before any user code runs.
    pub fn entry_allocas_push_store(&mut self, ty: crate::types::LlvmType, val: &str, ptr: &str) {
        self.entry_allocas
            .push(format!("  store {} {}, ptr {}", ty, val, ptr));
    }

    /// Emit a one-time void call in the function-entry setup region.
    ///
    /// Use this for metadata/registration work that must happen before
    /// any reachable hot-path use but does not need to run at each use
    /// site. If the function has an init prelude boundary, the call is
    /// spliced after runtime/string initialization; otherwise it is
    /// emitted at the top of the entry block with the other entry setup.
    pub fn entry_setup_call_void(&mut self, func_name: &str, args: &[(LlvmType, &str)]) {
        crate::ext_registry::record_ffi_call(func_name);
        let arg_str = args
            .iter()
            .map(|(ty, value)| format!("{} {}", ty, value))
            .collect::<Vec<_>>()
            .join(", ");
        let line = format!("  call void @{}({})", func_name, arg_str);
        self.entry_post_init_setup.push(line);
    }

    /// Emit a one-time function-entry init sequence: allocate a `ptr`
    /// slot, call `func_name()` (no args), store the result in the
    /// slot, return the slot pointer name. Used by the inline bump
    /// allocator to cache the per-thread `InlineArenaState` pointer
    /// once per JS function (instead of paying a TLS access on every
    /// `new ClassName()`).
    ///
    /// Lives in `entry_allocas` so the call + store run before any
    /// user code in the entry block, dominating every reachable use.
    /// The slot pointer is returned for the caller to load from at
    /// each subsequent allocation site.
    pub fn entry_init_call_ptr(&mut self, func_name: &str) -> String {
        let slot = self.alloca_entry(crate::types::PTR);
        let result_reg = format!("%r{}", self.reg_counter.next());
        self.entry_allocas
            .push(format!("  {} = call ptr @{}()", result_reg, func_name));
        self.entry_allocas
            .push(format!("  store ptr {}, ptr {}", result_reg, slot));
        slot
    }

    /// Emit a one-time function-entry load of a module global into a
    /// stack slot, returning the slot pointer. Used by the inline
    /// bump allocator to cache class-static values like the per-class
    /// `keys_array` global once per function instead of reloading it
    /// inside the hot allocation loop.
    ///
    /// LLVM's LICM should hoist a loop-invariant global load on its
    /// own, but doesn't when the loop body contains a call to an
    /// external function (like `js_inline_arena_slow_alloc`) that
    /// LLVM can't prove won't modify the global. Hoisting manually
    /// at the codegen layer sidesteps the alias-analysis question.
    pub fn entry_init_load_global(
        &mut self,
        global_name: &str,
        ty: crate::types::LlvmType,
    ) -> String {
        let slot = self.alloca_entry(ty);
        let result_reg = format!("%r{}", self.reg_counter.next());
        // The alloca dominates everything, but the load+store of the
        // global must run AFTER the entry-block init prelude (which is
        // what populates module-init globals like `@perry_class_keys_*`).
        // If a boundary has been marked, splice the load+store into
        // `entry_post_init_setup`; otherwise (no init prelude in this
        // function) we can put them right at the top with the alloca.
        let load_line = format!("  {} = load {}, ptr @{}", result_reg, ty, global_name);
        let store_line = format!("  store {} {}, ptr {}", ty, result_reg, slot);
        if self.entry_init_boundary.is_some() {
            self.entry_post_init_setup.push(load_line);
            self.entry_post_init_setup.push(store_line);
        } else {
            self.entry_allocas.push(load_line);
            self.entry_allocas.push(store_line);
        }
        slot
    }

    /// Create a new basic block with the given semantic name (e.g. "entry",
    /// "if.then"). A numeric suffix is appended to make the label unique
    /// across the function.
    pub fn create_block(&mut self, name: &str) -> &mut LlBlock {
        let label = format!("{}.{}", name, self.block_counter);
        self.block_counter += 1;
        let block = LlBlock::new_with_fp_flags(label, self.reg_counter.clone(), self.fp_flags);
        self.blocks.push(block);
        // Safe unwrap: we just pushed.
        self.blocks.last_mut().unwrap()
    }

    /// Accessor for an earlier block by index — needed when codegen has to
    /// come back and append to a predecessor (e.g. patching an unreachable
    /// fallthrough).
    pub fn block_mut(&mut self, idx: usize) -> Option<&mut LlBlock> {
        self.blocks.get_mut(idx)
    }

    pub fn blocks(&self) -> &[LlBlock] {
        &self.blocks
    }

    pub fn num_blocks(&self) -> usize {
        self.blocks.len()
    }

    /// Label of the last-created block — convenience for expression codegen
    /// that needs to feed a phi node the predecessor label after compiling a
    /// sub-expression whose control flow may have split.
    pub fn last_block_label(&self) -> Option<&str> {
        self.blocks.last().map(|b| b.label.as_str())
    }

    /// Cheap estimate of this function's rendered IR size in bytes, used to
    /// balance codegen-unit partitioning (#5391) without rendering twice. Sums
    /// the byte length of every instruction + entry alloca (the dominant terms);
    /// block labels/headers are a small fixed overhead per block.
    pub fn estimated_ir_bytes(&self) -> usize {
        let body: usize = self
            .blocks
            .iter()
            .map(|b| b.instructions_iter().map(|i| i.len() + 1).sum::<usize>() + b.label.len() + 4)
            .sum();
        let allocas: usize = self.entry_allocas.iter().map(|a| a.len() + 1).sum();
        body + allocas + self.name.len() + 64
    }

    pub fn to_ir(&self) -> String {
        let param_str = self
            .params
            .iter()
            .map(|(t, n)| format!("{} {}", t, n))
            .collect::<Vec<_>>()
            .join(", ");

        let linkage = if self.linkage.is_empty() {
            String::new()
        } else {
            format!("{} ", self.linkage)
        };

        let attrs = if self.has_try {
            // noinline (setjmp/volatile/async-rejecting boundary) always wins,
            // even if an inline attribute was optimistically set before body
            // lowering discovered the try.
            " #1"
        } else if self.force_inline {
            " alwaysinline"
        } else if self.inline_hint {
            " inlinehint"
        } else {
            ""
        };
        // The native-stack walker recovers frames through the x29 chain, so
        // every generated function must link one; without the attribute,
        // textual-IR input gets no frame-pointer default from the clang
        // driver and LLVM may omit the chain even while saving x29.
        let frame_pointer = if crate::codegen::helpers::native_stack_roots_enabled() {
            " \"frame-pointer\"=\"non-leaf\""
        } else {
            ""
        };
        let gc_strategy = if self.stack_map_requested
            && !self.has_try
            && (crate::codegen::helpers::statepoints_enabled()
                || crate::codegen::helpers::rs4gc_enabled())
        {
            " gc \"statepoint-example\""
        } else {
            ""
        };
        let mut ir = format!(
            "define {}{} @{}({}){}{}{} {{\n",
            linkage, self.return_type, self.name, param_str, attrs, frame_pointer, gc_strategy
        );

        for (i, blk) in self.blocks.iter().enumerate() {
            if i > 0 {
                ir.push('\n');
            }
            // Block 0 (entry) gets two splices in its body:
            //   1. `entry_allocas`: hoisted allocas + a few simple init
            //      sequences. These go at the very top, between the
            //      label line and any block instructions, so they
            //      dominate every reachable use in the function.
            //   2. `entry_post_init_setup`: hoisted setup that must
            //      run AFTER the init prelude (gc_init / init_strings
            //      calls) so it sees the up-to-date module state. The
            //      splice point is `entry_init_boundary`, which the
            //      codegen marks immediately after emitting the
            //      prelude.
            // Both splices are textual: we re-render the block label,
            // the prefix instructions (up to the boundary), the
            // post-init setup, and then the rest of the block body.
            if i == 0 && (!self.entry_allocas.is_empty() || !self.entry_post_init_setup.is_empty())
            {
                ir.push_str(&blk.label);
                ir.push_str(":\n");
                // 1. Allocas + simple inits at the very top.
                for alloca in &self.entry_allocas {
                    ir.push_str(alloca);
                    ir.push('\n');
                }
                // 2. Render the block instructions, with the post-init
                //    splice at the boundary index.
                let boundary = self
                    .entry_init_boundary
                    .unwrap_or(0)
                    .min(blk.instruction_count());
                let mut idx = 0;
                for inst in blk.instructions_iter() {
                    if idx == boundary {
                        for line in &self.entry_post_init_setup {
                            ir.push_str(line);
                            ir.push('\n');
                        }
                    }
                    ir.push_str(inst);
                    ir.push('\n');
                    idx += 1;
                }
                // Boundary at end-of-block (or empty block).
                if idx == boundary {
                    for line in &self.entry_post_init_setup {
                        ir.push_str(line);
                        ir.push('\n');
                    }
                }
            } else {
                ir.push_str(&blk.to_ir());
                ir.push('\n');
            }
        }

        ir.push_str("}\n");

        // Return-site rewrite hooks.
        //
        // Shadow-stack pop (gen-GC Phase A sub-phase 2) and entry
        // diagnostics both need to run before every normal return,
        // regardless of which lowering path emitted it. Textual rewrite
        // on the full IR catches implicit returns, Stmt::Return, and any
        // hand-emitted `ret`.
        let ir = if self.shadow_frame_slot.is_some() || !self.pre_return_void_calls.is_empty() {
            let mut out = String::with_capacity(ir.len() + 512);
            let mut seq: u32 = 0;
            for line in ir.lines() {
                let trimmed = line.trim_start();
                if (trimmed.starts_with("ret ") || trimmed == "ret void")
                    && !trimmed.starts_with("ret ptr ")
                // skip rare ptr rets
                {
                    for func_name in &self.pre_return_void_calls {
                        out.push_str(&format!("  call void @{}()\n", func_name));
                    }
                    if let Some(handle_slot) = &self.shadow_frame_slot {
                        let load_reg = format!("%shadow_pop_l_{}", seq);
                        seq += 1;
                        out.push_str(&format!("  {} = load i64, ptr {}\n", load_reg, handle_slot));
                        out.push_str(&format!(
                            "  call void @js_shadow_frame_pop(i64 {})\n",
                            load_reg
                        ));
                    }
                }
                out.push_str(line);
                out.push('\n');
            }
            out
        } else {
            ir
        };

        // Research backend: turn the existing shadow-slot binding IR into
        // native-frame stack maps only after lowering is complete, when every
        // lazily-reserved scalar root and every call site is visible.
        //
        let ir = if self.stack_map_requested {
            let backend = if crate::codegen::helpers::rs4gc_enabled() && !self.has_try {
                PreciseRootBackend::Rs4gc
            } else if crate::codegen::helpers::statepoints_enabled() && !self.has_try {
                PreciseRootBackend::Statepoint
            } else {
                PreciseRootBackend::StackMap
            };
            lower_precise_roots_to_native_stack(&ir, &self.name, self.stack_map_slot_count, backend)
        } else {
            ir
        };

        // setjmp volatile promotion (#6385).
        //
        // Runs LAST so it sees every instruction, including the ones the
        // return-site rewrite above just spliced in. Any alloca this function
        // stores into between a `setjmp` and its `longjmp` (recorded by
        // `LlBlock::emit` while a try region was open) gets `volatile` loads
        // and stores, which is what stops mem2reg/SROA from promoting it into
        // a register that `longjmp` would revert. This replaces the old
        // `optnone`-the-whole-function hammer.
        if self.has_try {
            let try_stores = self.reg_counter.try_region_stores();
            if !try_stores.is_empty() {
                return crate::volatile_setjmp::apply_setjmp_volatile(&ir, &try_stores);
            }
        }

        ir
    }
}

fn parse_shadow_bind(line: &str) -> Option<(usize, String)> {
    let rest = line
        .trim()
        .strip_prefix("call void @js_shadow_slot_bind(i32 ")?;
    let (idx, ptr) = rest.split_once(", ptr ")?;
    let ptr = ptr.strip_suffix(')')?.trim();
    Some((idx.parse().ok()?, ptr.to_string()))
}

fn parse_shadow_set(line: &str) -> Option<(usize, String)> {
    let rest = line
        .trim()
        .strip_prefix("call void @js_shadow_slot_set(i32 ")?;
    let (idx, value) = rest.split_once(", i64 ")?;
    let value = value.strip_suffix(')')?.trim();
    Some((idx.parse().ok()?, value.to_string()))
}

/// Compute a conservative set of active logical shadow slots before each IR
/// line. Joins use union ("active on any incoming path"), so a stale local can
/// be retained but a live root cannot be omitted.
fn stack_map_active_slots(
    lines: &[&str],
    slot_count: u32,
) -> Vec<Option<std::collections::HashSet<usize>>> {
    use std::collections::{HashMap, HashSet, VecDeque};

    #[derive(Debug)]
    struct Block {
        first_line: usize,
        end_line: usize,
        successors: Vec<usize>,
    }

    fn label_name(line: &str) -> Option<&str> {
        if line.starts_with(char::is_whitespace) {
            return None;
        }
        line.strip_suffix(':')
            .filter(|name| !name.is_empty() && !name.starts_with(';'))
    }

    fn referenced_labels(line: &str) -> Vec<&str> {
        let mut labels = Vec::new();
        let mut rest = line;
        while let Some(pos) = rest.find("label %") {
            let after = &rest[pos + "label %".len()..];
            let len = after
                .bytes()
                .take_while(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'$')
                })
                .count();
            if len == 0 {
                break;
            }
            labels.push(&after[..len]);
            rest = &after[len..];
        }
        labels
    }

    let labels: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter_map(|(idx, line)| label_name(line).map(|name| (idx, name)))
        .collect();
    let mut states = vec![None; lines.len()];
    if labels.is_empty() {
        return states;
    }

    let label_to_block: HashMap<&str, usize> = labels
        .iter()
        .enumerate()
        .map(|(block, (_, name))| (*name, block))
        .collect();
    let mut blocks: Vec<Block> = labels
        .iter()
        .enumerate()
        .map(|(block, (label_line, _))| Block {
            first_line: label_line + 1,
            end_line: labels
                .get(block + 1)
                .map_or(lines.len(), |(next_line, _)| *next_line),
            successors: Vec::new(),
        })
        .collect();
    for block in &mut blocks {
        let mut seen = HashSet::new();
        for line in &lines[block.first_line..block.end_line] {
            for label in referenced_labels(line) {
                if let Some(&successor) = label_to_block.get(label) {
                    if seen.insert(successor) {
                        block.successors.push(successor);
                    }
                }
            }
        }
    }

    fn apply_root_op(state: &mut HashSet<usize>, line: &str, slot_count: u32) {
        if let Some((idx, _)) = parse_shadow_bind(line) {
            if idx < slot_count as usize {
                state.insert(idx);
            }
        } else if let Some((idx, value)) = parse_shadow_set(line) {
            if idx < slot_count as usize {
                if value == "0" {
                    state.remove(&idx);
                } else {
                    state.insert(idx);
                }
            }
        }
    }

    let mut entries: Vec<Option<HashSet<usize>>> = vec![None; blocks.len()];
    entries[0] = Some(HashSet::new());
    let mut work = VecDeque::from([0usize]);
    while let Some(block_idx) = work.pop_front() {
        let Some(mut state) = entries[block_idx].clone() else {
            continue;
        };
        let block = &blocks[block_idx];
        for line in &lines[block.first_line..block.end_line] {
            apply_root_op(&mut state, line, slot_count);
        }
        for &successor in &block.successors {
            let changed = match &mut entries[successor] {
                Some(existing) => {
                    let old_len = existing.len();
                    existing.extend(state.iter().copied());
                    existing.len() != old_len
                }
                entry @ None => {
                    *entry = Some(state.clone());
                    true
                }
            };
            if changed {
                work.push_back(successor);
            }
        }
    }

    for (block_idx, block) in blocks.iter().enumerate() {
        let Some(mut state) = entries[block_idx].clone() else {
            continue;
        };
        for (line_idx, line) in lines
            .iter()
            .enumerate()
            .take(block.end_line)
            .skip(block.first_line)
        {
            states[line_idx] = Some(state.clone());
            apply_root_op(&mut state, line, slot_count);
        }
    }
    states
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreciseRootBackend {
    StackMap,
    Statepoint,
    /// `PERRY_RS4GC=1` (#7174): retype every root alloca to
    /// `ptr addrspace(1)` with cast surgery at its load/store sites, tag the
    /// function `gc "statepoint-example"`, mark audited non-collecting
    /// callees `"gc-leaf-function"` at the call site, and emit NO per-call
    /// safepoint machinery — `opt -passes='function(mem2reg),
    /// rewrite-statepoints-for-gc'` promotes the allocas to SSA and inserts
    /// every statepoint, relocation, and downstream-use rewrite itself.
    /// After mem2reg, each former load site is a cast site, which is exactly
    /// the placement RS4GC needs to rewrite uses with relocated values.
    /// Fail-closed: any use of a root alloca outside the recognized
    /// load/store shapes bails the whole function to the Statepoint backend.
    Rs4gc,
}

impl PreciseRootBackend {
    fn as_str(self) -> &'static str {
        match self {
            Self::StackMap => "stack-map",
            Self::Statepoint => "statepoint",
            Self::Rs4gc => "rs4gc",
        }
    }
}


/// RS4GC surgery (#7174): retype root allocas to `ptr addrspace(1)` and cast
/// at every recognized load/store site. Returns `None` when any root alloca
/// appears in an unrecognized shape (the caller falls back to the explicit
/// statepoint backend for the whole function).
fn lower_roots_for_rs4gc(
    lines: &[&str],
    root_ptrs: &[String],
) -> Option<String> {
    let roots: std::collections::HashSet<&str> = root_ptrs.iter().map(String::as_str).collect();
    let mut out = String::with_capacity(lines.len() * 48 + root_ptrs.len() * 96);
    let mut cast_counter = 0usize;

    for line in lines {
        if parse_shadow_bind(line).is_some() || parse_shadow_set(line).is_some() {
            continue;
        }
        let trimmed = line.trim_start();

        // Root-alloca definition: retype + null-init (mem2reg needs a
        // dominating definition for paths that read before the first bind,
        // same reason the i64 zero-init existed).
        // Root locals are emitted as `alloca double` (the NaN-box home) or
        // occasionally `alloca i64`; both become an addrspace(1) slot.
        if let Some(reg) = trimmed
            .strip_suffix("= alloca i64")
            .or_else(|| trimmed.strip_suffix("= alloca double"))
            .map(str::trim_end)
            .filter(|reg| roots.contains(reg))
        {
            out.push_str(&format!("  {reg} = alloca ptr addrspace(1)\n"));
            out.push_str(&format!("  store ptr addrspace(1) null, ptr {reg}\n"));
            continue;
        }

        let mut handled = false;
        for ptr in root_ptrs {
            if let Some(rest) = trimmed.strip_prefix("store i64 ") {
                if let Some(value) = rest.strip_suffix(&format!(", ptr {ptr}")) {
                    let value = value.trim();
                    if value == "0" {
                        out.push_str(&format!("  store ptr addrspace(1) null, ptr {ptr}\n"));
                    } else {
                        cast_counter += 1;
                        out.push_str(&format!(
                            "  %rs4gc.s{cast_counter} = inttoptr i64 {value} to ptr addrspace(1)\n  store ptr addrspace(1) %rs4gc.s{cast_counter}, ptr {ptr}\n"
                        ));
                    }
                    handled = true;
                    break;
                }
            }
            if let Some(rest) = trimmed.strip_prefix("store double ") {
                if let Some(value) = rest.strip_suffix(&format!(", ptr {ptr}")) {
                    let value = value.trim();
                    cast_counter += 1;
                    out.push_str(&format!(
                        "  %rs4gc.b{cast_counter} = bitcast double {value} to i64\n  %rs4gc.s{cast_counter} = inttoptr i64 %rs4gc.b{cast_counter} to ptr addrspace(1)\n  store ptr addrspace(1) %rs4gc.s{cast_counter}, ptr {ptr}\n"
                    ));
                    handled = true;
                    break;
                }
            }
            if trimmed == format!("{} = load i64, ptr {ptr}", trimmed.split(' ').next().unwrap_or("")) {
                let result = trimmed.split(' ').next().unwrap_or("");
                out.push_str(&format!(
                    "  {result}.rs4p = load ptr addrspace(1), ptr {ptr}\n  {result} = ptrtoint ptr addrspace(1) {result}.rs4p to i64\n"
                ));
                handled = true;
                break;
            }
            if trimmed == format!("{} = load double, ptr {ptr}", trimmed.split(' ').next().unwrap_or("")) {
                let result = trimmed.split(' ').next().unwrap_or("");
                out.push_str(&format!(
                    "  {result}.rs4p = load ptr addrspace(1), ptr {ptr}\n  {result}.rs4i = ptrtoint ptr addrspace(1) {result}.rs4p to i64\n  {result} = bitcast i64 {result}.rs4i to double\n"
                ));
                handled = true;
                break;
            }
        }
        if handled {
            continue;
        }

        // Fail closed: any other appearance of a root alloca name.
        if root_ptrs.iter().any(|ptr| {
            line.contains(ptr.as_str())
                && line
                    .split(|c: char| !(c.is_alphanumeric() || c == '%' || c == '_' || c == '.'))
                    .any(|tok| tok == ptr)
        }) {
            return None;
        }

        // Audited non-collecting callees become RS4GC leaf calls: the pass
        // will not treat them as safepoints, transferring the call-effect
        // table wholesale. AllocNoReentry keeps its contract gating.
        let is_call = trimmed.starts_with("call ")
            || trimmed.contains(" = call ")
            || trimmed.starts_with("tail call ")
            || trimmed.contains(" = tail call ");
        // Inline asm must be marked leaf explicitly: RS4GC otherwise rewrites
        // it into a statepoint whose callee is the asm value, which the
        // verifier rejects outright ("Cannot take the address of an inline
        // asm!"). Found on the Claude Code bundle, where other codegen paths
        // emit zero-instruction asm barriers.
        if is_call && trimmed.ends_with(')') && trimmed.contains(" asm ") {
            out.push_str(line.trim_end());
            out.push_str(" "gc-leaf-function"
");
            continue;
        }
        if is_call && trimmed.ends_with(')') && !trimmed.contains(" asm ") {
            if let Some(callee) = direct_callee_name(line) {
                let leaf = match crate::gc_call_effects::classify_direct_callee(callee) {
                    crate::gc_call_effects::GcCallEffect::CannotCollect
                    | crate::gc_call_effects::GcCallEffect::NeverReturns => true,
                    crate::gc_call_effects::GcCallEffect::AllocNoReentry => {
                        crate::codegen::helpers::gc_safepoint_only_contract_enabled()
                    }
                    crate::gc_call_effects::GcCallEffect::Unknown => false,
                };
                if leaf && !callee.starts_with("llvm.") {
                    out.push_str(line.trim_end());
                    out.push_str(" \"gc-leaf-function\"\n");
                    continue;
                }
            }
        }

        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}

#[derive(Debug, Eq, PartialEq)]
struct DirectCall<'a> {
    result: Option<&'a str>,
    return_type: &'a str,
    callee: &'a str,
    args: Vec<&'a str>,
    arg_types: Vec<&'a str>,
}

fn split_call_args(args: &str) -> Option<Vec<&str>> {
    if args.trim().is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (idx, ch) in args.char_indices() {
        match ch {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
            }
            ',' if depth == 0 => {
                out.push(args[start..idx].trim());
                start = idx + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    out.push(args[start..].trim());
    Some(out)
}

fn statepoint_scalar_type(arg: &str) -> Option<&str> {
    let ty = arg.split_ascii_whitespace().next()?;
    matches!(
        ty,
        "i1" | "i8" | "i16" | "i32" | "i64" | "i128" | "float" | "double" | "ptr"
    )
    .then_some(ty)
}

/// Parse the deliberately small direct-call subset emitted by `LlBlock`.
///
/// Calls with tail markers, operand attributes, aggregate types, inline asm,
/// indirect targets, or call-site suffixes stay on the plain stack-map
/// fallback. That keeps the research mode correct while making its explicit
/// statepoint coverage measurable and easy to expand.
fn parse_direct_statepoint_call(line: &str) -> Option<DirectCall<'_>> {
    let trimmed = line.trim();
    let (result, call) = if let Some(call) = trimmed.strip_prefix("call ") {
        (None, call)
    } else {
        let (result, call) = trimmed.split_once(" = call ")?;
        (Some(result.trim()), call)
    };
    let (return_type, target_and_args) = call.split_once(' ')?;
    if !matches!(
        return_type,
        "void" | "i1" | "i8" | "i16" | "i32" | "i64" | "i128" | "float" | "double" | "ptr"
    ) {
        return None;
    }
    if return_type != "void" && result.is_none() {
        return None;
    }
    let open = target_and_args.find('(')?;
    let close = target_and_args.rfind(')')?;
    if close + 1 != target_and_args.len() {
        return None;
    }
    let callee = target_and_args[..open].trim();
    if !callee.starts_with('@')
        || callee.starts_with("@llvm.")
        || matches!(callee, "@setjmp" | "@_setjmp" | "@longjmp" | "@_longjmp")
    {
        return None;
    }
    let args = split_call_args(&target_and_args[open + 1..close])?;
    let arg_types = args
        .iter()
        .map(|arg| statepoint_scalar_type(arg))
        .collect::<Option<Vec<_>>>()?;
    Some(DirectCall {
        result,
        return_type,
        callee,
        args,
        arg_types,
    })
}

/// Return a direct callee name without the leading `@`.
///
/// This accepts more call syntax than the statepoint parser because the
/// GC-effect audit only needs to recognize a direct target. Unsupported and
/// indirect forms return `None` and therefore stay conservative.
fn direct_callee_name(line: &str) -> Option<&str> {
    let call = line.trim().split_once("call ")?.1;
    let args_open = call.find('(')?;
    let target = call[..args_open].trim();
    let name = target.split_ascii_whitespace().last()?.strip_prefix('@')?;
    (!name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '$')))
    .then_some(name)
}

fn gc_result_suffix(ty: &str) -> Option<&'static str> {
    match ty {
        "i1" => Some("i1"),
        "i8" => Some("i8"),
        "i16" => Some("i16"),
        "i32" => Some("i32"),
        "i64" => Some("i64"),
        "i128" => Some("i128"),
        "float" => Some("f32"),
        "double" => Some("f64"),
        "ptr" => Some("p0"),
        _ => None,
    }
}

fn emit_plain_stack_map(out: &mut String, line: &str, live: &[&String], map_id: u64) {
    let operands = live
        .iter()
        .map(|ptr| format!(", ptr {ptr}"))
        .collect::<String>();
    out.push_str("  call void asm sideeffect \"\", \"~{memory}\"()\n");
    out.push_str(&format!(
        "  call void (i64, i32, ...) @llvm.experimental.stackmap(i64 {map_id}, i32 0{operands})\n"
    ));
    out.push_str(line);
    out.push('\n');
    out.push_str("  call void asm sideeffect \"\", \"~{memory}\"()\n");
}

/// Emit one explicit statepoint relocation sequence.
///
/// Perry roots remain ordinary NaN-boxed `i64` values everywhere else. At
/// this boundary we load each live word, carry its exact bits through a
/// temporary addrspace(1) pointer, and convert the `gc.relocate` result back
/// into the existing slot. LLVM therefore owns the spill/reload and the
/// post-safepoint SSA transition without requiring a whole-program
/// representation change for this prototype.
fn emit_statepoint(out: &mut String, call: &DirectCall<'_>, live: &[&String], statepoint_id: u64) {
    for (root_idx, ptr) in live.iter().enumerate() {
        out.push_str(&format!(
            "  %perry_sp_bits_{statepoint_id}_{root_idx} = load i64, ptr {ptr}\n"
        ));
        out.push_str(&format!(
            "  %perry_sp_root_{statepoint_id}_{root_idx} = inttoptr i64 \
             %perry_sp_bits_{statepoint_id}_{root_idx} to ptr addrspace(1)\n"
        ));
    }

    let function_type = format!("{} ({})", call.return_type, call.arg_types.join(", "));
    let call_args = call
        .args
        .iter()
        .map(|arg| format!(", {arg}"))
        .collect::<String>();
    let gc_live = live
        .iter()
        .enumerate()
        .map(|(root_idx, _)| format!("ptr addrspace(1) %perry_sp_root_{statepoint_id}_{root_idx}"))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!(
        "  %perry_sp_token_{statepoint_id} = call token (i64, i32, ptr, i32, i32, ...) \
         @llvm.experimental.gc.statepoint.p0(i64 {statepoint_id}, i32 0, \
         ptr elementtype({function_type}) {}, i32 {}, i32 0{call_args}, i32 0, i32 0) \
         [\"gc-live\"({gc_live})]\n",
        call.callee,
        call.args.len()
    ));

    if let Some(result) = call.result {
        let suffix = gc_result_suffix(call.return_type)
            .expect("non-void statepoint return type was validated by the parser");
        out.push_str(&format!(
            "  {result} = call {} @llvm.experimental.gc.result.{suffix}(token \
             %perry_sp_token_{statepoint_id})\n",
            call.return_type
        ));
    }

    for (root_idx, ptr) in live.iter().enumerate() {
        out.push_str(&format!(
            "  %perry_sp_relocated_{statepoint_id}_{root_idx} = call ptr addrspace(1) \
             @llvm.experimental.gc.relocate.p1(token %perry_sp_token_{statepoint_id}, \
             i32 {root_idx}, i32 {root_idx})\n"
        ));
        out.push_str(&format!(
            "  %perry_sp_relocated_bits_{statepoint_id}_{root_idx} = ptrtoint \
             ptr addrspace(1) %perry_sp_relocated_{statepoint_id}_{root_idx} to i64\n"
        ));
        out.push_str(&format!(
            "  store i64 %perry_sp_relocated_bits_{statepoint_id}_{root_idx}, ptr {ptr}\n"
        ));
    }
}

/// Lower Perry's existing precise-root operations to native-stack metadata.
///
/// The old binding calls already name exactly the mutable native alloca that a
/// moving collection must rewrite. We use them as compile-time markers:
///
/// * collect `logical slot -> native alloca`;
/// * remove the runtime bind calls and shadow-frame traffic;
/// * compute conservative per-call liveness from bind/clear markers without
///   mutating the native slot;
/// * either place a plain stack map before a call, or replace a supported call
///   with a statepoint/result/relocate sequence.
///
/// Statepoint mode deliberately retains a plain-stack-map fallback for call
/// forms outside the narrow parser above. The fallback preserves correctness
/// while the report records how much of real Perry code reaches the explicit
/// relocation path.
fn lower_precise_roots_to_native_stack(
    ir: &str,
    function_name: &str,
    slot_count: u32,
    backend: PreciseRootBackend,
) -> String {
    let lines: Vec<&str> = ir.lines().collect();
    let active_slots = stack_map_active_slots(&lines, slot_count);
    let mut roots: Vec<Option<String>> = vec![None; slot_count as usize];
    for line in &lines {
        if let Some((idx, ptr)) = parse_shadow_bind(line) {
            if let Some(root) = roots.get_mut(idx) {
                match root {
                    Some(existing) => {
                        debug_assert_eq!(
                            existing, &ptr,
                            "one precise-root slot must not bind two native allocas"
                        );
                    }
                    None => *root = Some(ptr),
                }
            }
        }
    }

    let slot_roots = roots;
    let root_ptrs: Vec<String> = slot_roots.iter().flatten().cloned().collect();
    let mut report = crate::statepoint_report::enabled().then(|| {
        crate::statepoint_report::FunctionRecord::new(
            function_name,
            backend.as_str(),
            slot_count,
            root_ptrs.len(),
        )
    });
    if root_ptrs.is_empty() {
        let out = ir
            .lines()
            .filter(|line| parse_shadow_bind(line).is_none() && parse_shadow_set(line).is_none())
            .map(|line| format!("{line}\n"))
            .collect();
        if let Some(report) = report {
            crate::statepoint_report::record(report);
        }
        return out;
    }

    if backend == PreciseRootBackend::Rs4gc {
        if let Some(out) = lower_roots_for_rs4gc(&lines, &root_ptrs) {
            if let Some(mut report) = report {
                report.note_call(root_ptrs.len());
                crate::statepoint_report::record(report);
            }
            return out;
        }
        // A root alloca is used in a shape the surgery does not recognize —
        // fail closed to the explicit statepoint backend for this function.
        return lower_precise_roots_to_native_stack(
            ir,
            function_name,
            slot_count,
            PreciseRootBackend::Statepoint,
        );
    }

    let mut out = String::with_capacity(ir.len() + root_ptrs.len() * 128);
    let mut available = std::collections::HashSet::<String>::new();
    let mut initialized = std::collections::HashSet::<String>::new();
    let mut map_id = 0u64;

    for (line_idx, line) in lines.iter().enumerate() {
        if parse_shadow_bind(line).is_some() {
            // Compile-time marker only. The real slot is already populated by
            // the local store immediately preceding this old bind.
            continue;
        }
        if parse_shadow_set(line).is_some() {
            // This marker changes stack-map liveness, not the program local.
            // Shadow-stack clears only flipped SLOT_ACTIVE for the same
            // reason: a value can be semantically read after its final
            // GC-capable call.
            continue;
        }

        // A stack-map operand must dominate the intrinsic. Root allocas are
        // normally entry-hoisted, but tracking definitions here also handles
        // the few block-local scalar-replacement slots without emitting
        // invalid SSA.
        for ptr in &root_ptrs {
            if line.trim_start().starts_with(&format!("{ptr} = ")) {
                available.insert(ptr.clone());
            }
        }

        out.push_str(line);
        out.push('\n');

        // Slots can be named by a stack map before their source-level `let`
        // executes. Zero them directly after the alloca so an earlier
        // safepoint never exposes uninitialized stack bytes as roots.
        for ptr in &root_ptrs {
            if available.contains(ptr)
                && !initialized.contains(ptr)
                && line.trim_start().starts_with(&format!("{ptr} = alloca "))
            {
                out.push_str(&format!("  store i64 0, ptr {ptr}\n"));
                initialized.insert(ptr.clone());
            }
        }


        // Insert before calls, not after. Rebuild the tail when the line just
        // appended is a call so the intrinsic's instruction offset is the
        // actual call-site offset in the final machine function.
        let trimmed = line.trim_start();
        let is_call = trimmed.starts_with("call ")
            || trimmed.contains(" = call ")
            || trimmed.starts_with("tail call ")
            || trimmed.contains(" = tail call ");
        if !is_call || trimmed.contains("@llvm.experimental.stackmap") {
            continue;
        }
        let active = active_slots.get(line_idx).and_then(Option::as_ref);
        let live: Vec<&String> = slot_roots
            .iter()
            .enumerate()
            .filter(|(idx, _)| active.is_some_and(|slots| slots.contains(idx)))
            .filter_map(|(_, ptr)| ptr.as_ref())
            .filter(|ptr| available.contains(*ptr) && initialized.contains(*ptr))
            .collect();
        if let Some(report) = report.as_mut() {
            report.note_call(live.len());
        }
        if live.is_empty() {
            continue;
        }

        let direct_callee = direct_callee_name(line);
        let is_compiler_only = direct_callee.is_some_and(|callee| callee.starts_with("llvm."))
            || trimmed.contains("call void asm ");
        let cannot_collect = direct_callee.is_some_and(|callee| {
            match crate::gc_call_effects::classify_direct_callee(callee) {
                crate::gc_call_effects::GcCallEffect::CannotCollect => true,
                // Control never returns here: no relocation is consumed and
                // the frame's roots are dead past the call. Deeper frames
                // carry their own records.
                crate::gc_call_effects::GcCallEffect::NeverReturns => true,
                // Under the explicit-safepoint contract the runtime
                // guarantees these helpers' triggers never consume this
                // frame's precise roots (they defer to a declared safepoint
                // or collect behind a forced conservative scan), so the
                // call site needs no metadata. Without the contract they
                // stay safepoints.
                crate::gc_call_effects::GcCallEffect::AllocNoReentry => {
                    crate::codegen::helpers::gc_safepoint_only_contract_enabled()
                }
                crate::gc_call_effects::GcCallEffect::Unknown => false,
            }
        });
        if is_compiler_only || cannot_collect {
            // LLVM intrinsics, zero-instruction compiler barriers, and
            // runtime helpers in the audited GC-effect table cannot enter
            // Perry's allocator. Neither native-stack backend needs metadata
            // around them.
            if let Some(report) = report.as_mut() {
                report.note_skipped(direct_callee.unwrap_or("<inline-asm>"));
            }
            continue;
        }

        // Move the call line behind the intrinsic.
        let call_len = line.len() + 1;
        out.truncate(out.len() - call_len);
        if backend == PreciseRootBackend::Statepoint {
            if let Some(call) = parse_direct_statepoint_call(line) {
                emit_statepoint(&mut out, &call, &live, map_id);
                if let Some(report) = report.as_mut() {
                    report.note_statepoint(call.callee.trim_start_matches('@'), live.len());
                }
                map_id += 1;
                continue;
            }
        }
        emit_plain_stack_map(&mut out, line, &live, map_id);
        if let Some(report) = report.as_mut() {
            report.note_plain_stack_map(
                direct_callee.unwrap_or("<indirect-or-unsupported>"),
                live.len(),
                backend == PreciseRootBackend::Statepoint,
            );
        }
        map_id += 1;
    }
    if let Some(report) = report {
        crate::statepoint_report::record(report);
    }
    out
}

#[cfg(test)]
mod stack_map_tests {
    use super::{
        direct_callee_name, lower_precise_roots_to_native_stack, parse_direct_statepoint_call,
        PreciseRootBackend,
    };

    fn lower_stack_maps(input: &str, slots: u32) -> String {
        lower_precise_roots_to_native_stack(input, "probe", slots, PreciseRootBackend::StackMap)
    }

    fn lower_statepoints(input: &str, slots: u32) -> String {
        lower_precise_roots_to_native_stack(input, "probe", slots, PreciseRootBackend::Statepoint)
    }

    #[test]
    fn lowers_bind_and_liveness_clear_to_native_frame_maps() {
        let input = r#"define i64 @probe(i64 %arg) {
entry.0:
  %r0 = alloca i64
  store i64 %arg, ptr %r0
  call void @js_shadow_slot_bind(i32 0, ptr %r0)
  %r1 = call i64 @may_collect()
  call void @js_shadow_slot_set(i32 0, i64 0)
  call void @may_collect_again()
  ret i64 %r1
}
"#;
        let output = lower_stack_maps(input, 1);
        assert!(!output.contains("@js_shadow_slot_bind"));
        assert!(!output.contains("@js_shadow_slot_set"));
        assert!(output.contains("%r0 = alloca i64\n  store i64 0, ptr %r0"));
        assert!(output.contains(
            "@llvm.experimental.stackmap(i64 0, i32 0, ptr %r0)\n  %r1 = call i64 \
             @may_collect()\n  call void asm sideeffect \"\", \"~{memory}\"()"
        ));
        assert_eq!(output.matches("store i64 0, ptr %r0").count(), 1);
        assert_eq!(output.matches("@llvm.experimental.stackmap").count(), 1);
        assert!(output.contains("call void @may_collect_again()"));
    }

    #[test]
    fn does_not_reference_a_root_before_its_alloca_dominates() {
        let input = r#"define void @probe() {
entry.0:
  call void @early_call()
  %r0 = alloca i64
  call void @js_shadow_slot_bind(i32 0, ptr %r0)
  call void @late_call()
  ret void
}
"#;
        let output = lower_stack_maps(input, 1);
        let early = output.find("call void @early_call()").unwrap();
        let first_map = output.find("@llvm.experimental.stackmap").unwrap();
        assert!(early < first_map);
        assert!(output.contains(
            "@llvm.experimental.stackmap(i64 0, i32 0, ptr %r0)\n  call void @late_call()"
        ));
    }

    #[test]
    fn unions_root_liveness_at_control_flow_joins() {
        let input = r#"define void @probe(i1 %cond) {
entry.0:
  %r0 = alloca i64
  call void @js_shadow_slot_bind(i32 0, ptr %r0)
  br i1 %cond, label %live.1, label %dead.2
live.1:
  call void @live_call()
  br label %merge.3
dead.2:
  call void @js_shadow_slot_set(i32 0, i64 0)
  call void @dead_call()
  br label %merge.3
merge.3:
  call void @merge_call()
  ret void
}
"#;
        let output = lower_stack_maps(input, 1);
        assert!(output.contains(
            "@llvm.experimental.stackmap(i64 0, i32 0, ptr %r0)\n  call void @live_call()"
        ));
        assert!(!output.contains(
            "@llvm.experimental.stackmap(i64 1, i32 0, ptr %r0)\n  call void @dead_call()"
        ));
        assert!(output.contains(
            "@llvm.experimental.stackmap(i64 1, i32 0, ptr %r0)\n  call void @merge_call()"
        ));
    }

    #[test]
    fn parses_the_scalar_direct_call_subset() {
        assert_eq!(
            direct_callee_name("  %r7 = call double @foo(i64 %r1, ptr %r2)"),
            Some("foo")
        );
        assert_eq!(
            direct_callee_name("  %r7 = call i64 ()* %fn()"),
            None,
            "an indirect target must not be inferred from its arguments"
        );
        assert_eq!(
            parse_direct_statepoint_call("  %r7 = call double @foo(i64 %r1, ptr %r2)"),
            Some(super::DirectCall {
                result: Some("%r7"),
                return_type: "double",
                callee: "@foo",
                args: vec!["i64 %r1", "ptr %r2"],
                arg_types: vec!["i64", "ptr"],
            })
        );
        assert!(parse_direct_statepoint_call(
            "  %r7 = call double (i64, ptr)* %fn(i64 %r1, ptr %r2)"
        )
        .is_none());
        assert!(parse_direct_statepoint_call("  call void @llvm.assume(i1 %ok)").is_none());
        assert!(parse_direct_statepoint_call("  %r7 = tail call i64 @foo()").is_none());
    }

    #[test]
    fn lowers_direct_calls_to_explicit_statepoint_relocations() {
        let input = r#"define i64 @probe(i64 %arg) {
entry.0:
  %r0 = alloca i64
  store i64 %arg, ptr %r0
  call void @js_shadow_slot_bind(i32 0, ptr %r0)
  %r1 = call i64 @may_collect(i64 %arg)
  ret i64 %r1
}
"#;
        let output = lower_statepoints(input, 1);
        assert!(!output.contains("call i64 @may_collect"));
        assert!(!output.contains("asm sideeffect"));
        assert!(output
            .contains("%perry_sp_root_0_0 = inttoptr i64 %perry_sp_bits_0_0 to ptr addrspace(1)"));
        assert!(output.contains(
            "ptr elementtype(i64 (i64)) @may_collect, i32 1, i32 0, i64 %arg, i32 0, i32 0"
        ));
        assert!(output
            .contains("%r1 = call i64 @llvm.experimental.gc.result.i64(token %perry_sp_token_0)"));
        assert!(output
            .contains("@llvm.experimental.gc.relocate.p1(token %perry_sp_token_0, i32 0, i32 0)"));
        assert!(output.contains("store i64 %perry_sp_relocated_bits_0_0, ptr %r0"));
    }

    #[test]
    fn statepoint_mode_falls_back_for_indirect_calls() {
        let input = r#"define i64 @probe(i64 %arg, ptr %fn) {
entry.0:
  %r0 = alloca i64
  store i64 %arg, ptr %r0
  call void @js_shadow_slot_bind(i32 0, ptr %r0)
  %r1 = call i64 ()* %fn()
  ret i64 %r1
}
"#;
        let output = lower_statepoints(input, 1);
        assert!(output.contains("@llvm.experimental.stackmap(i64 0, i32 0, ptr %r0)"));
        assert!(output.contains("%r1 = call i64 ()* %fn()"));
        assert!(!output.contains("@llvm.experimental.gc.statepoint"));
    }

    #[test]
    fn statepoint_mode_does_not_map_non_allocating_llvm_intrinsics() {
        let input = r#"define void @probe(i64 %arg, i1 %condition) {
entry.0:
  %r0 = alloca i64
  store i64 %arg, ptr %r0
  call void @js_shadow_slot_bind(i32 0, ptr %r0)
  call void @llvm.assume(i1 %condition)
  call void @may_collect()
  ret void
}
"#;
        let output = lower_statepoints(input, 1);
        assert!(output.contains("call void @llvm.assume(i1 %condition)"));
        assert_eq!(
            output
                .matches("@llvm.experimental.gc.statepoint.p0")
                .count(),
            1
        );
        assert!(!output.contains("@llvm.experimental.stackmap"));
    }

    #[test]
    fn audited_non_collecting_helpers_are_not_safepoints_in_either_backend() {
        let input = r#"define void @probe(i64 %arg) {
entry.0:
  %r0 = alloca i64
  store i64 %arg, ptr %r0
  call void @js_shadow_slot_bind(i32 0, ptr %r0)
  call void @js_gc_temp_root_push(i64 %arg)
  call void @js_write_barrier_root_nanbox(i64 %arg)
  call void @js_gc_loop_safepoint()
  ret void
}
"#;
        for output in [lower_stack_maps(input, 1), lower_statepoints(input, 1)] {
            assert!(output.contains("call void @js_gc_temp_root_push(i64 %arg)"));
            assert!(output.contains("call void @js_write_barrier_root_nanbox(i64 %arg)"));
            assert_eq!(
                output.matches("@llvm.experimental.stackmap").count()
                    + output
                        .matches("@llvm.experimental.gc.statepoint.p0")
                        .count(),
                1,
                "only the explicit collection boundary should be a safepoint:\n{output}"
            );
        }
    }
}
