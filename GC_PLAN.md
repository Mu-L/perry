Use **V8 Orinoco as Perry’s canonical performance reference**, but do **not** copy V8’s full architecture literally. Perry’s best target is:

**young generation:** V8-style **parallel Scavenger**
**old generation:** Orinoco-style **incremental/concurrent marking + concurrent sweeping + selective parallel compaction**
**compiler contract:** LLVM/statepoint- or stackmap-grade **precise roots**, with no conservative root scanning in the normal path
**implementation reference:** MMTk **GenImmix / ConcurrentImmix** as a practical Rust-friendly design library, not as a drop-in solution.

That gives Perry the right north star: **generational, mostly moving, mostly precise, increasingly concurrent**.

## The core diagnosis

Perry’s current GC is already on the right broad track: it has a nursery/old split, a copying fast path, survivor spaces, promotion, write barriers, shadow-stack roots, old-gen evacuation policy, and a 128 MB trigger ceiling. But its performance ceiling is bounded by several architectural constraints: copying minor GC is single-threaded, major GC is stop-the-world, roots are hybrid precise/conservative, the remembered set is page-granular, evacuation is policy-gated, and concurrent marking is explicitly parked as future work.

The biggest gap from V8 is not “Perry uses mark-sweep.” The bigger gap is that V8 moved a lot of GC work off the main mutator pause: parallel young collection, concurrent major marking, concurrent sweeping, parallel compaction, and parallel pointer updates. V8’s own Orinoco writeup distinguishes parallel, incremental, and concurrent GC work, and describes V8’s current young collection as parallel scavenging and major GC as concurrent marking/sweeping plus parallel compaction/pointer updating. ([V8][1])

## The most important recommendation

Do **not** make “concurrent MinorMS” Perry’s first big target.

Your V8 breakdown is correct that V8 now has Scavenger, Minor Mark-Sweep, and Mark-Compact surfaces. But for Perry, the first high-return target should be a **parallel copying young collector**, not concurrent young mark-sweep. V8’s public docs say the young generation uses a parallel Scavenger that copies live objects, uses forwarding pointers, and uses thread-local allocation buffers for surviving objects; V8 reported roughly 20%–50% reduction in young-generation GC main-thread time from parallel Scavenger work across benchmarks. ([V8][1]) ([V8][2])

V8’s `minor_ms` is still exposed as an experimental feature flag in current source, while `concurrent_minor_ms_marking` is tied to that path. That makes MinorMS a useful reference for a later alternative mode, but not the canonical first architecture for Perry. ([Chromium Git Repositories][3])

## Priority 1: make the write barrier cheap enough to keep always-on

Perry’s barrier currently fires on every heap store emitted by codegen, decodes parent/child, checks old→young, and dirties the page containing the slot. That is the correct semantic idea, but it is too expensive if it remains an out-of-line runtime call on the common path. The file explicitly calls this a hot path that fires on every heap store.

The fix is to split the barrier into an **inlined fast path** and a **runtime slow path**.

The inlined fast path should do only:

```text
if child is not heap pointer: return
if parent is not heap pointer: return
if parent is not old: return
if child is not young: return
dirty_card(slot)
```

Only the final dirtying step should call into runtime when needed. Better still, the common dirtying path should be a direct store to a card-table byte or bitmap, not a hash-set insertion.

Perry currently tracks dirty old pages with `DIRTY_OLD_PAGES`, a page-granular `PtrHashSet`, plus a separate external slot-page map.  That is simple and correct, but for performance Perry should move toward:

```text
4 KB dirty page set        -> coarse fallback
512 B / 1 KB card table    -> normal old→young tracking
typed slot buffers         -> compacting / precise pointer update
external slot cards        -> Map.entries, buffers, native side storage
```

This is likely one of the highest-throughput wins because it improves performance even when GC is not actively collecting.

## Priority 2: eliminate conservative scanning from the normal path

Perry’s current root story is hybrid: precise shadow stack for compiled JS frames, plus conservative C-stack/register scanning for Rust runtime frames. The conservative path uses `setjmp`, and on AArch64 explicitly captures FP registers because LLVM may hold NaN-boxed pointers there.

That is a correctness safety net, but it is a moving/concurrent GC tax. Conservative roots cannot be safely rewritten. Perry therefore pins objects discovered conservatively, which directly limits evacuation and compaction. The file also notes that conservative scan discoveries can populate `CONS_PINNED` because false positives cannot be rewritten safely.

The target should be:

```text
normal execution: fully precise roots only
debug / emergency mode: conservative scan allowed
production moving/concurrent mode: no conservative roots on the hot path
```

For compiled TypeScript, Perry can either keep improving the existing shadow-stack design or move toward LLVM statepoints/stack maps. LLVM’s GC docs say LLVM does not provide the collector itself, but does provide compiler support for stack maps at call sites, safepoint polling, and load/store barriers; the Statepoints docs describe mechanisms intended to support precise, fully relocating collectors. ([LLVM][4]) ([LLVM][5])

For Rust runtime code, Perry should enforce a handle discipline:

```rust
HandleScope
GcHandle<T>
MutableGcSlot<T>
RootedJsValue
```

No raw `JSValue` should live across an allocation or safepoint unless it is registered in a mutable root slot. That is the price of a high-performance moving collector.

## Priority 3: make the copying minor path nearly unconditional

Perry already has the right young-generation shape: Eden, survivor spaces, old space, forwarding, promotion, and reference rewriting. The problem is that `gc_collect_minor_with_trigger` first tries the copying fast path, then falls back to mark-sweep when copying is ineligible because of conservative stack activity, inactive barriers, pinned young roots, or similar reasons.

The target should be:

```text
minor GC = copying Scavenger path >99% of the time
fallback full mark-sweep = diagnostic / rare emergency path
```

That requires:

1. Barriers always emitted and always correct.
2. Runtime roots converted to mutable handles.
3. Conservative scanning disabled in normal GC mode.
4. Pinned young objects handled as pinned objects/pages, not as a reason to abandon the whole copying collector.
5. External malloc-backed structures integrated into the same root/slot-update model.

A full mark-sweep fallback inside frequent minor GC defeats the main value of generational GC. Young GC should be bounded by **live young data + remembered-set roots**, not by the whole heap.

## Priority 4: parallelize the young collector before adding concurrent young marking

Perry’s current young copying collector should evolve into a parallel Scavenger.

The design should look roughly like this:

```text
stop the mutator
main thread scans precise roots
worker threads scan dirty cards / remembered slots
all workers copy live young objects using CAS forwarding
each worker allocates into local survivor / promotion buffers
global work queues support stealing
join workers
flip survivor spaces
resume mutator
```

V8’s parallel Scavenger uses forwarding pointers, compare-and-swap synchronization, and thread-local allocation buffers for survivors. ([V8][1]) Perry already has forwarding machinery and survivor/old allocation paths, so this is a natural extension, not a redesign.

This should precede concurrent young marking because copying nursery GC is usually ideal for JavaScript/TypeScript allocation patterns: many objects die young, and collecting only live young objects is cheap. Concurrent MinorMS is more attractive when object movement is hard, when a unified heap complicates copying, or when young live sets are too large to copy efficiently. Those are second-order problems for Perry.

## Priority 5: reduce the nursery trigger for latency mode; separate latency and throughput policies

Perry’s trigger policy currently has a 128 MB initial threshold / ceiling, with commentary showing it was raised from 64 MB to 128 MB to avoid mid-benchmark GC events in allocation-heavy cases.

That is defensible for throughput benchmarks, but it is not a good universal latency default. A 128 MB stop-the-world nursery can create visible pauses if the live set is high, roots are numerous, or remembered scanning is expensive.

Perry should have at least two policies:

```text
PERRY_GC_MODE=latency
  smaller nursery target, e.g. 8–32 MB
  pause-budget-driven trigger
  more frequent minor collections
  earlier concurrent major marking

PERRY_GC_MODE=throughput
  larger nursery target, e.g. 64–128 MB
  fewer collections
  tolerate larger pauses
  optimize total runtime
```

Also, minor-GC triggers should be based primarily on **young allocation debt**, not total arena size. Major-GC triggers should be based on **old live growth**, fragmentation, RSS pressure, and allocation rate.

## Priority 6: add Orinoco-style concurrent major marking

After young GC is parallel and root precision is solved, the next major step is old-gen concurrent marking.

V8’s public concurrent-marking writeup says concurrent marking allows JavaScript to continue while the GC scans the heap, and reports 60%–70% reduction in main-thread marking time in V8 benchmarks. It also explains why a write barrier is required when the mutator can change the object graph while marking is in progress. ([V8][6])

For Perry, the architecture should be:

```text
when old-gen allocation debt crosses threshold:
  activate marking barrier
  start background marking workers

during execution:
  mutator performs incremental marking steps at safepoints/allocation debt
  background workers drain marking worklists
  write barrier preserves tri-color invariant

final pause:
  stop mutator briefly
  rescan roots
  finish ephemerons / weak refs / finalizers
  publish marking results
  deactivate marking barrier
```

One caveat: V8’s public blog describes a Dijkstra-style barrier; the exact barrier family Perry chooses can be incremental-update, SATB, or a hybrid. The important point is not the label. The important point is that Perry needs a **marking barrier active only during incremental/concurrent marking**, separate from the always-on generational old→young barrier.

## Priority 7: add concurrent sweeping before full old-gen compaction

Concurrent sweeping is much easier than concurrent compaction and gives a large practical win.

After old-gen marking finishes, Perry should:

```text
sweep old pages concurrently
return empty pages to OS in background
rebuild free lists in background
let allocation lazily sweep a page if it needs memory before background sweeping catches up
```

V8’s Orinoco design starts concurrent sweeping tasks during the pause, and those tasks may continue after JavaScript resumes. ([V8][1])

This is a better near-term target than always-compacting old gen. Compaction requires precise relocation of every root and every heap slot. Sweeping mostly requires correct mark bits, page ownership, and allocation synchronization.

## Priority 8: make old-gen compaction selective, parallel, and page-based

Perry already has policy-gated old-gen evacuation based on pressure, pause budget, tenured bytes still in nursery, and old-page fragmentation.  That is the correct policy shape. The missing piece is making it more systematic and more parallel.

The target is:

```text
do not compact all old gen
select fragmented pages
evacuate selected pages in parallel
record typed slots for pointer updates
update roots and heap slots precisely
abort evacuation for pinned/native-conservative pages
sweep non-evacuated pages concurrently
```

This mirrors the spirit of Orinoco without requiring Perry to clone every V8 detail.

## Priority 9: use shape/layout metadata to make tracing cheap

Perry should not trace objects by generic dynamic dispatch if it can avoid it. AOT compilation and known object layouts are Perry’s advantage.

Each heap object should have either:

```text
type id -> trace descriptor
shape id -> pointer bitmap / slot layout
array kind -> element pointer policy
native object kind -> custom scanner
```

Then tracing becomes:

```text
load descriptor
scan only pointer fields
skip primitive-only shapes
bulk-scan dense object arrays
avoid string/int/float fields entirely
```

This also helps remembered-set processing, compaction pointer updates, and concurrent marking. V8’s marking writeup notes that outgoing edges can be found using object metadata such as hidden classes. ([V8][6]) Perry should use its own shape/layout descriptors in the same conceptual role.

## Priority 10: reduce allocation pressure before overengineering GC

The fastest GC work is the work Perry never creates.

Because Perry is AOT, it should be more aggressive than V8 on compile-time allocation elimination:

```text
escape analysis
stack allocation for non-escaping objects
scalar replacement of object literals
closure allocation elimination
array bounds/shape specialization
pretenuring for module-level and long-lived closures
string builder / rope lowering for concat-heavy code
JSON parse/stringify transient arenas
short-lived promise/continuation pooling where semantics allow
```

This is where Perry can outperform a JIT runtime in controlled TypeScript code. V8 adapts after seeing runtime behavior; Perry can remove whole allocation families before the program runs.

## What to use as the canonical reference

Use **three references**, each for a different layer.

**1. Canonical JS-runtime GC architecture: V8 Orinoco.**
This should be Perry’s main architectural reference because it is optimized for JavaScript object graphs, generational behavior, write barriers, weak references, moving objects, and dynamic object layouts. Specifically, copy the pattern: parallel young collection, concurrent/incremental major marking, concurrent sweeping, selective parallel compaction. ([V8][1])

**2. Canonical young-gen design: V8 parallel Scavenger, not MinorMS.**
Perry’s current young collector is already copying-oriented, so the closest high-performance target is V8’s parallel Scavenger. Treat V8 MinorMS as a later research/reference path for sticky-mark-bit or unified-heap scenarios, not as the first rewrite. ([V8][2])

**3. Canonical implementation/policy library: MMTk GenImmix / ConcurrentImmix.**
MMTk is not a drop-in answer for Perry, but it is a strong reference for how modern collectors are decomposed into plans, spaces, allocators, work packets, barriers, and VM bindings. MMTk documents GenImmix as a generational collector with an evacuating nursery and Immix mature space, and ConcurrentImmix as a concurrent collector using SATB with opportunistic defragmentation. ([Memory Management Toolkit][7])

## What not to use as the canonical model

Do not use **Boehm GC** as the performance reference. Conservative non-moving GC is useful for C/C++ integration, but it would lock Perry into false retention, weak compaction, and poor object-movement semantics.

Do not use **Go’s GC** as the main model. Go’s collector is excellent for Go’s constraints, but it is not generational and not optimized around JavaScript’s short-lived object churn.

Do not use **ZGC/Shenandoah** as the first target. They are sophisticated low-latency collectors, but colored/load-barrier moving collectors would be far more complex than Perry needs right now.

Do not use **V8 MinorMS** as the first Perry target. It is useful and relevant, but Perry’s current design and likely allocation profile make parallel copying young GC the more direct win.

## Concrete target architecture for Perry

A reasonable “Perry GC vNext” target would be:

```text
Collector:
  generational
  copying parallel young collector
  Immix-like or mark-region old space
  concurrent old-gen marking
  concurrent old-gen sweeping
  selective old-gen evacuation/compaction

Roots:
  precise compiled-code roots
  precise runtime handles
  conservative scanning only in debug/emergency mode

Barriers:
  inlined old→young card barrier, always on
  marking barrier, active only during incremental/concurrent marking
  typed slot recording for compaction candidates

Remembered sets:
  card table for old→young references
  typed slot buffers for evacuation/pointer update
  external-slot cards for malloc-backed side storage

Policy:
  young allocation debt for minor GC
  old live-growth factor for major GC
  pause-budget modes
  RSS pressure mode
  idle/background GC hooks

AOT optimizations:
  escape analysis
  scalar replacement
  stack allocation
  pretenuring
  shape-specific tracing
```

## Recommended implementation order

First, instrument everything: pause p50/p95/p99, barrier calls versus slow hits, copied bytes, promoted bytes, live young bytes, remembered cards scanned, root-scan time, conservative roots found, pinned bytes, fallback causes, old fragmentation, and allocation by site/type/shape.

Second, inline the barrier and replace the dirty-page hash set with a card table. This improves mutator throughput immediately.

Third, eliminate conservative scanning from the normal path by moving runtime `JSValue` lifetimes into explicit handles and making compiled roots fully precise.

Fourth, make the copying minor path nearly unconditional. A minor GC fallback to full mark-sweep should be treated as a bug or rare emergency path.

Fifth, parallelize the young collector. This is the closest Perry equivalent to V8’s proven Scavenger win.

Sixth, implement concurrent old-gen marking with a real marking barrier.

Seventh, add concurrent sweeping.

Eighth, add selective parallel old-gen compaction.

Ninth, add AOT allocation elimination so the compiler reduces GC load before runtime.

## Bottom line

The canonical target for Perry should be:

**V8 Orinoco’s architecture, V8 Scavenger’s young-gen strategy, LLVM’s precise-GC compiler contract, and MMTk GenImmix/ConcurrentImmix as a practical implementation reference.**

For drastic performance improvement, Perry should not jump straight to the most advanced concurrent young collector. It should first make the existing generational design cheap, precise, mostly-moving, and parallel. Then add Orinoco-style concurrency for old-gen work.

[1]: https://v8.dev/blog/trash-talk "Trash talk: the Orinoco garbage collector · V8"
[2]: https://v8.dev/blog/orinoco-parallel-scavenger "Orinoco: young generation garbage collection · V8"
[3]: https://chromium.googlesource.com/v8/v8/%2B/master/src/flags/flag-definitions.h "src/flags/flag-definitions.h - v8/v8 - Git at Google"
[4]: https://llvm.org/docs/GarbageCollection.html "Garbage Collection with LLVM — LLVM 23.0.0git documentation"
[5]: https://llvm.org/docs/Statepoints.html "Garbage Collection Safepoints in LLVM — LLVM 23.0.0git documentation"
[6]: https://v8.dev/blog/concurrent-marking "Concurrent marking in V8 · V8"
[7]: https://www.mmtk.io/status "Memory Management Toolkit"
