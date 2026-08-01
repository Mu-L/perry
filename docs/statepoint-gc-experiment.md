# Explicit statepoint GC experiment

Date: 2026-07-31

Branch: `exp/stackmap-viability`

Base commit: `e2557c1a985cb983ed00aafd1a2c1b31f1570b98`

## Decision

The explicit `gc.statepoint` bridge is correct enough to validate the
mechanism, but it is not currently a performance win and does not yet make
Perry's GC simpler. Keep the shadow stack as the default.

- All eight GC-ratchet probes pass normally and with forced evacuation plus
  evacuation verification.
- The full suite emits 1,080 statepoints and 1,562 relocations. It has zero
  plain-stack-map fallbacks at GC-relevant calls.
- Runtime is effectively flat versus the shadow stack: -0.27% geometric mean
  on a heavily loaded host. It is 1.42% slower than the plain-stack-map arm.
- Uncached compilation is 2.12% slower than shadow-stack compilation.
- Statepoint stack-map payload is 2.01x the plain-map payload across the suite.
- Relocation is better expressed: LLVM now owns the call/result/relocated-value
  relationship, and the compiler memory barriers from the plain-map prototype
  disappear.
- The whole system remains more complex because native unwinding, stack-map
  parsing, textual call rewriting, root liveness, fallbacks, and
  platform-specific metadata retention are still required.

The prototype remains opt-in with `PERRY_STATEPOINTS=1`. The default
shadow-stack path is unchanged.

## Follow-up: root pressure and audited safepoints

The first prototype treated almost every textual call with live roots as a
safepoint. That was correct but needlessly pessimistic. The follow-up adds an
audited GC-call-effect table whose only claim is whether a helper can enter
Perry's collector. Unknown calls remain safepoints.

This is intentionally separate from LLVM memory effects. Temporary-root
bookkeeping, write barriers, layout notes, feedback counters, and refcount
writes mutate memory, but they do not run a Perry collection and therefore do
not need stack-map metadata.

`--statepoint-report[=json]` makes the resulting root pressure visible. It
reports per-function logical/bound root slots, calls with live roots, audited
non-collecting calls, statepoints, relocations, plain-map fallbacks, live-root
widths, and callee frequencies. It is observational and disables cache reuse
for the reporting run:

```sh
PERRY_STATEPOINTS=1 perry compile app.ts --statepoint-report
```

On `benchmarks/app-patterns/kernels/batch.ts`, the audit changed:

| Metric | Before | After | Change |
|---|---:|---:|---:|
| Statepoints | 442 | 219 | -50.5% |
| Relocations | 867 | 403 | -53.5% |
| `__llvm_stackmaps` | 54,968 B | 26,432 B | -51.9% |
| Plain-map fallbacks | 0 | 0 | unchanged |

The report found 223 calls with live roots that cannot collect. The largest
groups were typed-feedback bookkeeping, class-field guards, temporary-root
push/get/truncate, layout notes, write barriers, and property-observation
records.

Across the eight GC probes:

| Probe | Statepoints before | Statepoints after | Relocations after | Calls skipped |
|---|---:|---:|---:|---:|
| Nursery churn | 152 | 65 | 91 | 88 |
| Survivor promotion | 165 | 79 | 132 | 88 |
| Cross-generation writes | 168 | 74 | 95 | 96 |
| Dead after deep stack | 119 | 55 | 59 | 65 |
| Closure capture | 146 | 69 | 81 | 78 |
| String retention | 92 | 45 | 46 | 50 |
| Array grow/evacuate | 100 | 49 | 49 | 52 |
| Map/set side tables | 138 | 66 | 124 | 74 |
| **Total** | **1,080** | **502** | **677** | **591** |

The same audit applies to the plain-map backend. Its eight-probe metadata
payload is now 26,816 bytes; explicit statepoints use 53,952 bytes. Statepoint
metadata therefore remains 2.01x plain maps even after both shrink by roughly
half. Reducing the number of maybe-pointer roots through representation
promotion remains the larger shared lever.

All eight probes pass in shadow, plain-map, and statepoint modes with forced
evacuation and relocation verification: 24/24 mode/probe comparisons match
Node's probe/checksum output.

### Follow-up runtime and compile measurements

Each runtime cell below is the median of seven executions on the same host and
full-feature runtime artifact. Host variance was high, so the numbers are
directional:

| Probe | Shadow | Plain stack map | Statepoint | Statepoint vs plain |
|---|---:|---:|---:|---:|
| Nursery churn | 196.698 ms | 194.860 ms | 195.178 ms | +0.16% |
| Survivor promotion | 234.184 ms | 235.600 ms | 233.294 ms | -0.98% |
| Cross-generation writes | 383.002 ms | 330.386 ms | 341.206 ms | +3.27% |
| Dead after deep stack | 1,029.803 ms | 996.121 ms | 1,196.806 ms | +20.15% |
| Closure capture | 230.904 ms | 212.415 ms | 188.984 ms | -11.03% |
| String retention | 151.553 ms | 173.651 ms | 162.272 ms | -6.55% |
| Array grow/evacuate | 251.063 ms | 267.094 ms | 237.260 ms | -11.17% |
| Map/set side tables | 577.092 ms | 563.401 ms | 601.368 ms | +6.74% |

Geometric means put plain maps at -1.17% versus shadow, statepoints at -1.54%
versus shadow, and statepoints at -0.38% versus plain maps. That small aggregate
statepoint lead is below the noise floor; the deep-stack regression remains a
clear negative signal.

For uncached `batch.ts` compilation, seven-run medians were 910.3 ms shadow,
946.0 ms plain maps (+3.92%), and 958.3 ms statepoints (+5.28%).

### Follow-up: x29-chain fast walker (2026-07-31, after rebase onto #7114)

The deep-stack telemetry below (36,458 frames unwound for 104 root
locations) identified `_Unwind_Backtrace` as the walker bottleneck: full
compact-unwind register recovery on every native frame. The branch now walks
the raw x29 chain instead — two loads per frame — enabled by two facts:

1. Generated functions now carry `"frame-pointer"="non-leaf"`. Textual-IR
   input gets no frame-pointer default from the clang driver, which is why
   generated code previously saved x29 without establishing a chain.
2. Statepoint spills are SP-relative (`Indirect [R#31 + N]`) on AArch64
   regardless of frame-pointer attributes, but the stack-map header records
   each function's frame size, and LLVM's AArch64 frame keeps the
   `[x29, x30]` pair at the top of the frame — so the body SP is always
   `fp + 16 - stack_size` from the same chain loads.

The fast path is fail-closed twice over. At parse time, any location that is
not FP-relative or sized-SP-relative marks the whole image not chain-walkable.
At walk time, a misaligned, non-increasing, or out-of-bounds frame pointer
abandons the walk and re-runs the platform unwinder (slot visits are
idempotent, so a partial fast walk followed by a full unwinder walk is safe).
`PERRY_STACKMAP_WALKER=unwind` forces the old walker as a bisection control;
`PERRY_STACKMAP_WALKER=verify` runs both and panics unless they visit the
identical slot set. Verify exists because forced-evacuation verification
enumerates roots through the same walker and therefore cannot catch a walker
that silently skips frames — this is CLAUDE.md gate-failure mode 4 applied to
the walker itself. Telemetry gains `fp_walks`/`fallback_walks` so any run can
prove which walker executed.

Results (loaded host, directional): 24/24 correctness matrix, 16/16
verify-mode probe runs (fast walk engaged and byte-identical to the unwinder
everywhere), and the deep-stack statepoint probe improves 4.8% end-to-end
against the unwinder walker on the same binary with interleaved reps.

A finding for the mode decision fell out of the register census: plain-map
mode emits `Register R#1` locations — the root slot's address materialized in
a caller-saved register at the map point. No parser can soundly use that
location (the register is clobbered by the callee and unrecoverable at GC
time), so those roots are structurally invisible to the collector. LLVM's
stackmap intrinsic offers no way to force the address into memory; statepoint
spill slots cannot exhibit the problem. Plain maps are therefore unsound by
construction at a small but nonzero rate (3 of 60 locations on the deep-stack
probe), which strengthens the case for deleting the plain-map arm once
statepoints match it on the walker-sensitive workloads.

### Native-root and unwinder telemetry

Native stack-map roots now have their own `root_sources.compiled_native`
telemetry bucket instead of being incorrectly charged to
`compiled_shadow`. `root_sources.native_stack_maps` also records walks, frames
visited, records matched, and locations visited.

The forced-evacuation deep-stack probe reported 105 walks, 36,458 frames
visited, 36,139 records matched, and only 104 root locations visited, with a
maximum of 694 frames in one cycle. The walker is therefore a justified
optimization target, but a direct frame-pointer walker is not yet a safe
substitution: current generated AArch64 code saves `x29` without consistently
establishing an `x29` frame chain, and Rust/runtime frames have no matching
contract. A fast path first needs an explicit frame-pointer/unwind ABI for
generated and intervening runtime frames, plus fallback and cross-architecture
tests.

### Follow-up: the explicit-safepoint collection contract (PERRY_GC_SAFEPOINT_ONLY)

The prerequisite that gated this experiment — the #7114 temp-root
correctness fix — landed on main during the first prototype session, so the
contract experiment ran after rebasing onto it.

**The contract.** A collection that skips the conservative stack scan
consumes only precise roots; with native stack maps active, precise frame
roots exist only at mapped PCs. Therefore such a collection may only begin
at a declared safepoint (a loop back-edge poll or the outermost
microtask-pump boundary) — anywhere else it must scan conservatively. The
runtime already routes moving minors to those safepoints (the #7024
deferral machinery), so today the property is *emergent*: it holds because
every possibly-collecting call happens to be mapped. The contract makes it
*enforced* — a thread-local declared-safepoint flag plus a check at the
root-scan subphase — and enforcement is what makes it sound to stop mapping
call sites.

Two enforcement levels: `PERRY_GC_SAFEPOINT_ONLY=1` (heal — an undeclared
precise-root cycle gets the conservative scan forced for that cycle, which
restores liveness and keeps it non-moving) and `=strict` (panic — the gate
mode that proves the enforcement is live, per the four-ways-a-gate-cannot-
fail rule). Manual `gc()` and the alloc-point slack valve force the scan
already and are exempt by construction. (An earlier revision also drained
non-nursery triggers at every allocating loop back-edge; that turned churn
loops into per-iteration collection work — O(n²) — and was deleted. The heal
alone is sufficient: undeclared full collections simply pay one conservative
scan.)

**What it unmaps.** A new audited `GcCallEffect::AllocNoReentry` class:
helpers that may allocate (arming a trigger) but never collect synchronously
and never re-enter generated JS. Under the contract their call sites need no
statepoint — any trigger they arm either defers to a declared safepoint or
collects behind the forced scan. First audited set: singleton closure
allocation, class-object allocation, `js_array_push_f64`, `js_array_length`,
`js_array_slice_values`.

**The census result that bounds the idea.** On `batch.ts`, 217 statepoints
break down as roughly 85 property-access diamonds (getter re-entry possible
— must stay mapped), ~40 coercion/setter/throw paths (re-entry — stay), ~10
generated-to-generated calls and polls (stay by definition), and only ~25-30
pure-allocation sites the contract can unmap. **Re-entry, not allocation, is
what bounds the contract's reach on object-heavy code.** Deleting the
property-access calls is representation selection's job (`Ptr<Shape>`); the
contract unmaps what allocation traffic remains. The two campaigns compose
rather than compete.

### Work deliberately left gated

This follow-up does not alter temporary-root semantics, collection scheduling,
or the conservative native-stack fallback. The representation plan makes the
temp-root correctness work a prerequisite for an explicit-only collection
contract and conservative-scanner removal. Doing that here would overlap the
other agent's work and make failures impossible to attribute. Once that
prerequisite lands, the next experiment is to assert that moving collections
occur only at declared safepoints and then measure whether the conservative
scanner can be deleted.

## Which statepoint design this tests

This is the explicit bridge, not LLVM's `RewriteStatepointsForGC` pipeline.
Perry emits the three intrinsics directly:

1. Load each live NaN-boxed `i64` root from its existing native alloca and
   temporarily convert the bits to `ptr addrspace(1)`.
2. Replace the original call with `llvm.experimental.gc.statepoint`.
3. Recover the call's scalar return through `llvm.experimental.gc.result`.
4. Recover every live root through `llvm.experimental.gc.relocate`, convert it
   back to `i64`, and store it to the original alloca.

The runtime collector executes inside the statepoint's callee. It unwinds to
the generated caller, finds LLVM's `Indirect` spill locations in
`__LLVM_STACKMAPS`, and rewrites those words during evacuation. The generated
caller then reloads the rewritten words through `gc.relocate`.

The small standalone version is
[`statepoint-bridge-probe.ll`](statepoint-bridge-probe.ll).

This choice intentionally avoids colliding with the representation experiment.
A full `RewriteStatepointsForGC` integration wants managed pointers to be
identifiable throughout SSA and expects a compiler pass to discover and
rewrite safepoints. Perry currently carries GC-capable values as NaN-boxed
`i64` words, so the bridge changes their representation only across one call.

## Why LLVM calls it experimental

The `llvm.experimental.*` prefix means LLVM does not promise a permanently
stable IR or binary interface across releases. It does not mean that the
mechanism is an abandoned toy or that production-oriented runtimes cannot use
it. For Perry it creates an engineering requirement: pin and test supported
LLVM versions, verify emitted IR, and treat upgrades as an ABI migration.

The relevant upstream contracts are
[Garbage collection safepoints](https://llvm.org/docs/Statepoints.html) and
[Stack maps and patch points](https://llvm.org/docs/StackMaps.html).

## Implementation

The prototype reuses the precise-root discovery and conservative per-call CFG
liveness built for the plain-stack-map experiment.

- Functions with roots receive `gc "statepoint-example"`.
- Ordinary direct calls with scalar arguments and scalar/void results are
  rewritten explicitly.
- LLVM intrinsics and compiler-only inline assembly are not safepoints.
- Unsupported call forms retain the plain `llvm.experimental.stackmap`
  fallback.
- A function containing Perry's setjmp-based `try` lowering uses the plain-map
  backend for the whole function.
- The runtime parser accepts plain-map `Direct` alloca addresses and statepoint
  `Indirect` spill locations. It deduplicates identical base/derived locations
  before visiting roots.
- The module retains each Mach-O `__LLVM_STACKMAPS` atom with
  `.no_dead_strip`.
- `PERRY_STATEPOINTS` participates in both build and object cache keys.

When `PERRY_STATEPOINTS=1` and `PERRY_STACK_MAPS=1` are both present,
statepoints take precedence in eligible functions.

## Correctness and coverage

Final release artifacts passed:

- all 8 GC-ratchet probes with stdout identical to Node;
- all 8 probes with `PERRY_GC_FORCE_EVACUATE=1` and
  `PERRY_GC_VERIFY_EVACUATION=1`;
- LLVM 22 verification of all eight generated modules;
- compilation of the minimal bridge with Apple clang 21.0.0 and LLVM 22.1.4;
- 314 `perry-codegen` library tests;
- 5 focused runtime stack-map/statepoint parser and call-site tests;
- the object-cache statepoint environment-key test.

Final generated-IR coverage:

| Probe | Statepoints | Relocations | Plain fallbacks |
|---|---:|---:|---:|
| Nursery churn | 152 | 227 | 0 |
| Survivor promotion | 165 | 296 | 0 |
| Cross-generation writes | 168 | 244 | 0 |
| Dead after deep stack | 119 | 135 | 0 |
| Closure capture | 146 | 191 | 0 |
| String retention | 92 | 95 | 0 |
| Array grow/evacuate | 100 | 100 | 0 |
| Map/set side tables | 138 | 274 | 0 |
| **Total** | **1,080** | **1,562** | **0** |

The zero here describes these probes, not the backend's complete call-form
coverage. Indirect calls, aggregate signatures, unusual call-site attributes,
and setjmp functions can still take the deliberate plain-map fallback.

Retained heap and heap capacity match the shadow-stack arm byte-for-byte.
Promotions, freed bytes, and cycle counts also match. Two probes have tiny
copy-accounting differences (+3 objects/+208 bytes and -152 bytes), with
identical final retention; the other six match all checked GC counters.

One apparent intermittent relocation failure during development was a stale
`target/*/libperry_runtime.a`. Perry executables link the
`perry-runtime-static` package, not the `perry-runtime` rlib directly.
Rebuilding only the latter left the old scanner in generated binaries. The
final results rebuild both the compiler and static runtime archive.

## Performance

Hardware was an Apple M1 Max on macOS 26.5. The interleaved run reported load
averages of 22.71/25.83/27.77, so these results are directional and should not
be promoted to release claims.

Each runtime cell is the median of 11 executions. Mode order was interleaved
and rotated after one warmup, and outputs were checked for equality before
timing.

| Probe | Shadow | Plain stack map | Statepoint | Statepoint vs plain |
|---|---:|---:|---:|---:|
| Nursery churn | 182.026 ms | 177.968 ms | 175.573 ms | -1.35% |
| Survivor promotion | 216.247 ms | 208.667 ms | 208.192 ms | -0.23% |
| Cross-generation writes | 210.743 ms | 203.775 ms | 204.504 ms | +0.36% |
| Dead after deep stack | 460.249 ms | 470.665 ms | 501.690 ms | +6.59% |
| Closure capture | 173.719 ms | 163.076 ms | 162.902 ms | -0.11% |
| String retention | 130.773 ms | 133.166 ms | 136.794 ms | +2.72% |
| Array grow/evacuate | 171.915 ms | 175.039 ms | 172.519 ms | -1.44% |
| Map/set side tables | 460.879 ms | 444.031 ms | 466.627 ms | +5.09% |

Geometric means versus shadow:

- plain stack maps: -1.66%;
- statepoints: -0.27%;
- statepoints versus plain maps: +1.42%.

The deep-stack result is the strongest negative signal. Both native-stack
backends pay for unwinding, while statepoints additionally materialize
relocation spill/reload state around a large number of calls.

Three uncached compilations per probe measured a +1.47% geometric mean for
plain maps and +2.12% for statepoints versus shadow. Sequential RSS
measurements put statepoints at roughly +1.03% median RSS and +0.70% peak RSS,
but allocator and host noise make those figures less reliable than retained
heap.

Statepoint metadata is materially larger:

| Probe | Plain payload | Statepoint payload |
|---|---:|---:|
| Nursery churn | 7,104 B | 13,656 B |
| Survivor promotion | 8,224 B | 16,112 B |
| Cross-generation writes | 7,768 B | 15,040 B |
| Dead after deep stack | 5,128 B | 14,824 B |
| Closure capture | 9,464 B | 18,440 B |
| String retention | 6,016 B | 10,912 B |
| Array grow/evacuate | 5,840 B | 11,480 B |
| Map/set side tables | 7,288 B | 13,840 B |

The total is 114,304 bytes versus 56,832 bytes, or 2.01x. Most executables are
about 16 KiB larger than shadow after Mach-O segment rounding; closure capture
crosses another segment boundary and is about 33 KiB larger.

## Is the GC simpler?

Relocation is simpler to reason about, but the GC system is not simpler yet.

The improvement is real: a statepoint makes the original call result and every
post-call root explicit SSA results. Plain maps required empty inline assembly
memory barriers to stop LLVM from caching root values across a call whose
stack slots the compiler could not know the collector mutates.

However, this bridge still needs:

- Perry's root discovery, logical slots, and conservative CFG liveness;
- addressable root allocas and per-statepoint load/store bridges;
- the LLVM stack-map v3 parser and native unwinder;
- Mach-O section discovery and linker-retention directives;
- call-form parsing and plain-map fallbacks;
- target- and toolchain-specific verification.

It removes generated shadow-frame push/pop and TLS slot mutation, but replaces
that local machinery with a wider compiler/linker/runtime contract. The
collector itself is nearly unchanged; only its root source changes.

## Is Perry better positioned?

Semantically, yes. Operationally, not enough yet to switch.

Statepoints provide the right vocabulary for a future moving collector:
relocation is explicit, base/derived relationships have a representation, and
a later managed-pointer pipeline can keep roots in SSA rather than forcing
Perry to invent compiler barriers.

This implementation is still a bridge with important liabilities:

- arbitrary NaN-boxed bits temporarily masquerade as managed pointers;
- all ordinary calls with live roots are treated as potentially allocating;
- indirect and unusual calls fall back to plain maps;
- `try`/setjmp functions fall back wholesale;
- scanning is macOS/Mach-O-only;
- active-frame matching still uses a 16-byte nearest-PC tolerance;
- the intrinsic and metadata contracts require LLVM-version discipline;
- the current measurements show no all-around speedup.

## Recommended next step

Do not replace the shadow stack from this branch. Preserve the prototype as
evidence and wait for the representation work before choosing the production
path.

After that work lands:

1. Define a safepoint-capability table so only calls that can enter the
   allocator become statepoints.
2. Represent genuine managed references directly instead of converting every
   possible NaN-box root through `inttoptr`.
3. Compare direct explicit emission with `RewriteStatepointsForGC` on that
   representation.
4. Remove plain-map fallbacks one call form at a time and fail closed on
   unsupported targets.
5. Re-run the 11-way interleaved suite on an idle pinned host, with separate
   profiles for mutator root maintenance, relocation reloads, unwinding, and
   collector root scanning.
