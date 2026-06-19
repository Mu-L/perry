## V8 Orinoco GC: Current Surface and Evolution

The GC surface is organized around three collectors, each with distinct parallelism strategies. The "Orinoco" label is still used explicitly in the flag section header.

---

### Collector Inventory

| Collector | Class | Scope | File |
|---|---|---|---|
| Scavenger | `ScavengerCollector` | Young gen (semi-space) | `src/heap/scavenger.cc` |
| Minor Mark-Sweep | `MinorMarkSweepCollector` | Young gen (mark-sweep) | `src/heap/minor-mark-sweep.cc` |
| Mark-Compact | `MarkCompactCollector` | Full heap | `src/heap/mark-compact.cc` |

The young-generation collector is selected by `--minor-ms`. The Scavenger is the default; `MinorMarkSweepCollector` is the newer alternative. This is the primary **divergence point** in the GC surface.

---

### Phase-by-Phase Parallelism

#### Major GC (`MarkCompactCollector`)

**1. Marking** — three overlapping modes:

- **Incremental** (`IncrementalMarking`): breaks marking into small steps interleaved with JS execution. Steps are triggered by allocation observers (`kMajorGCYoungGenerationAllocationObserverStep = 64 KB`, `kMajorGCOldGenerationAllocationObserverStep = 256 KB`) and a background task. [1](#0-0)

- **Concurrent** (`ConcurrentMarking`): background threads drain the marking worklist while the mutator runs. Requires `V8_ATOMIC_OBJECT_FIELD_WRITES`. Up to 7 workers by default (`concurrent_marking_max_worker_num`). Scheduled via `V8::GetCurrentPlatform()->PostJob()`. [2](#0-1)

- **Parallel** (in atomic pause): `parallel_marking` flag re-schedules the concurrent marking job at `kUserBlocking` priority during the stop-the-world pause to drain remaining work. [3](#0-2)

The full marking sequence in `MarkLiveObjects()` is: finish incremental → mark roots → parallel transitive closure fixpoint → serial transitive closure (single-threaded, for weak maps/embedder heap safety). [4](#0-3)

**2. Clearing** (`ClearNonLiveReferences`): uses a `ParallelClearingJob` with a dependency-graph of `ParallelItem` tasks (string table chunks, weak refs, map transitions, etc.) dispatched concurrently. Controlled by `parallel_gc_clearing`. [5](#0-4) [6](#0-5)

**3. Sweeping** (`Sweeper`): concurrent background sweeping via `ConcurrentMajorSweeper` / `MajorSweeperJob`. The main thread starts sweeping and immediately returns; background tasks continue. Controlled by `concurrent_sweeping`. [7](#0-6) [8](#0-7)

**4. Evacuation/Compaction**: parallel via `PageEvacuationJob` with `NumberOfParallelCompactionTasks()` evacuators. Controlled by `parallel_compaction`. [9](#0-8) [10](#0-9)

**5. Pointer update**: parallel via `parallel_pointer_update`. [11](#0-10)

---

#### Minor GC — Scavenger (`ScavengerCollector`)

Parallel stop-the-world semi-space copy. A `ScavengerJobTask` is posted before root iteration; background threads process old-to-new remembered set pages while the main thread iterates roots in parallel. Controlled by `parallel_scavenge`. **No concurrent (out-of-pause) phase.** [12](#0-11) [13](#0-12)

---

#### Minor GC — Minor Mark-Sweep (`MinorMarkSweepCollector`)

This is where the **divergence from the Scavenger** lies. MinorMS supports a concurrent marking phase outside the atomic pause:

- `concurrent_minor_ms_marking` flag enables incremental+concurrent marking for the young generation. `StartMinorMSConcurrentMarkingIfNeeded()` triggers it when new space exceeds `minor_ms_min_new_space_capacity_for_concurrent_marking_mb`. [14](#0-13)

- `ConcurrentMarking::RunMinor()` dispatches `RunMinorImpl<kConcurrent>` outside the pause and `RunMinorImpl<kParallel>` inside it, using `YoungGenerationMarkingVisitationMode` to switch behavior. [15](#0-14)

- `MinorMarkSweepCollector::MarkLiveObjects()` checks `was_marked_incrementally` to decide whether to start fresh or finalize an ongoing concurrent cycle. [16](#0-15)

- Sweeping is concurrent via `ConcurrentMinorSweeper` / `MinorSweeperJob`. [7](#0-6)

---

### The Write Barrier: Enabling Concurrent Marking

`MarkingBarrier` (per `LocalHeap`) is the mechanism that makes concurrent marking safe. It prevents black-to-white references by intercepting stores during incremental/concurrent marking. It supports both `kMajorMarking` and `kMinorMarking` modes and publishes local worklists to the global pool at safepoints. [17](#0-16) [18](#0-17)

---

### The Orinoco Flag Surface

The flags are explicitly grouped under `// Parallel and concurrent GC (Orinoco) related flags.` in `flag-definitions.h`. `single_threaded_gc` is the master kill-switch:

```
single_threaded_gc disables:
  concurrent_marking, concurrent_sweeping, parallel_compaction,
  parallel_marking, parallel_pointer_update, parallel_gc_clearing,
  parallel_scavenge, concurrent_array_buffer_sweeping,
  stress_concurrent_allocation, cppheap_concurrent_marking
``` [19](#0-18)

Individual defaults (all `true` on capable platforms): [20](#0-19)

Background-mode nuance: `single_threaded_gc_in_background` (default `true`) disables background GC tasks when the isolate is in efficiency mode, but `parallel_pause_for_gc_in_background` and `incremental_marking_for_gc_in_background` preserve parallel work inside the atomic pause. [21](#0-20) [22](#0-21)

---

### Where the Surface Diverged

```mermaid
graph TD
  "Sequential STW\n(original)" --> "Incremental Marking\n(IncrementalMarking, 2012)"
  "Incremental Marking\n(IncrementalMarking, 2012)" --> "Concurrent Marking\n(ConcurrentMarking, 2017)"
  "Concurrent Marking\n(ConcurrentMarking, 2017)" --> "Parallel Scavenge\n(ScavengerJobTask)"
  "Concurrent Marking\n(ConcurrentMarking, 2017)" --> "Minor MS\n(MinorMarkSweepCollector, 2023)"
  "Minor MS\n(MinorMarkSweepCollector, 2023)" --> "Concurrent Minor Marking\nconcurrent_minor_ms_marking"
  "Parallel Scavenge\n(ScavengerJobTask)" --> "Parallel STW only\nno out-of-pause concurrency"
```

The key divergence is:
1. **Scavenger** remains a **parallel stop-the-world** collector — it gained parallel workers but never gained a concurrent (out-of-pause) phase.
2. **MinorMarkSweepCollector** (introduced with `minor-mark-sweep.h` copyright 2023) is the branch that gained **concurrent marking for the young generation**, mirroring what the major GC had since ~2017.
3. The major GC's incremental marking (`IncrementalMarking`, copyright 2012 in the header) predates the concurrent marking infrastructure (`ConcurrentMarking`, copyright 2017), and the two are composed: incremental steps run on the main thread while concurrent tasks run in the background, both draining the same `MarkingWorklists`. [23](#0-22) [24](#0-23) [25](#0-24)

### Citations

**File:** src/heap/incremental-marking.cc (L52-58)
```text
static constexpr size_t kMajorGCYoungGenerationAllocationObserverStep = 64 * KB;
static constexpr size_t kMajorGCOldGenerationAllocationObserverStep = 256 * KB;

static constexpr v8::base::TimeDelta kMaxStepSizeOnTask =
    v8::base::TimeDelta::FromMilliseconds(1);
static constexpr v8::base::TimeDelta kMaxStepSizeOnAllocation =
    v8::base::TimeDelta::FromMilliseconds(5);
```

**File:** src/heap/concurrent-marking.cc (L579-615)
```text
void ConcurrentMarking::RunMinor(JobDelegate* delegate) {
  DCHECK(heap_->use_new_space());
  DCHECK_NOT_NULL(heap_->new_lo_space());
  uint8_t task_id = delegate->GetTaskId() + 1;
  DCHECK_LT(task_id, task_state_.size());
  TaskState* task_state = task_state_[task_id].get();
  double time_ms;
  size_t marked_bytes = 0;
  Isolate* isolate = heap_->isolate();
  if (v8_flags.trace_concurrent_marking) {
    isolate->PrintWithTimestamp("Starting minor concurrent marking task %d\n",
                                task_id);
  }

  {
    TimedScope scope(&time_ms);
    if (heap_->minor_mark_sweep_collector()->is_in_atomic_pause()) {
      // This gets a lower bound for estimated concurrency as we may have marked
      // most of the graph concurrently already and may not be using parallism
      // as much.
      estimate_concurrency_.fetch_add(1, std::memory_order_relaxed);
      marked_bytes =
          RunMinorImpl<YoungGenerationMarkingVisitationMode::kParallel>(
              delegate, task_state);
    } else {
      marked_bytes =
          RunMinorImpl<YoungGenerationMarkingVisitationMode::kConcurrent>(
              delegate, task_state);
    }
  }

  if (v8_flags.trace_concurrent_marking) {
    heap_->isolate()->PrintWithTimestamp(
        "Minor task %d concurrently marked %dKB in %.2fms\n", task_id,
        static_cast<int>(marked_bytes / KB), time_ms);
  }
}
```

**File:** src/heap/concurrent-marking.cc (L703-713)
```text
  if (garbage_collector == GarbageCollector::MARK_COMPACTOR) {
    heap_->mark_compact_collector()->local_marking_worklists()->Publish();
    marking_worklists_ = heap_->mark_compact_collector()->marking_worklists();
    auto job = std::make_unique<JobTaskMajor>(
        this, heap_->mark_compact_collector()->epoch(),
        heap_->mark_compact_collector()->code_flush_mode(),
        heap_->ShouldCurrentGCKeepAgesUnchanged());
    current_job_trace_id_.emplace(job->trace_id());
    TRACE_GC_NOTE_WITH_FLOW("Major concurrent marking started", job->trace_id(),
                            TRACE_EVENT_FLAG_FLOW_OUT);
    job_handle_ = V8::GetCurrentPlatform()->PostJob(priority, std::move(job));
```

**File:** src/heap/mark-compact.cc (L2585-2672)
```text
void MarkCompactCollector::MarkLiveObjects() {
  TRACE_GC_ARG1(heap_->tracer(), GCTracer::Scope::MC_MARK,
                "UseBackgroundThreads", UseBackgroundThreadsInCycle());

  const bool was_marked_incrementally =
      !heap_->incremental_marking()->IsStopped();
  if (was_marked_incrementally) {
    auto* incremental_marking = heap_->incremental_marking();
    TRACE_GC_WITH_FLOW(
        heap_->tracer(), GCTracer::Scope::MC_MARK_FINISH_INCREMENTAL,
        incremental_marking->current_trace_id(), TRACE_EVENT_FLAG_FLOW_IN);
    DCHECK(incremental_marking->IsMajorMarking());
    incremental_marking->Stop();
    MarkingBarrier::PublishAll(heap_);

    // Incremental marking might leave ephemerons in main task's local
    // buffer, flush it into global pool.
    local_weak_objects()->next_ephemerons_local.Publish();
  }

#ifdef DEBUG
  DCHECK(state_ == PREPARE_GC);
  state_ = MARK_LIVE_OBJECTS;
#endif

  if (heap_->cpp_heap_) {
    CppHeap::From(heap_->cpp_heap_)
        ->EnterFinalPause(heap_->embedder_stack_state_);
  }

  RootMarkingVisitor root_visitor(this);

  {
    TRACE_GC(heap_->tracer(), GCTracer::Scope::MC_MARK_ROOTS);
    MarkRoots(&root_visitor);
  }

  {
    TRACE_GC(heap_->tracer(), GCTracer::Scope::MC_MARK_CLIENT_HEAPS);
    MarkObjectsFromClientHeaps();
  }

  {
    TRACE_GC(heap_->tracer(), GCTracer::Scope::MC_MARK_RETAIN_MAPS);
    RetainMaps();
  }

  if (v8_flags.parallel_marking && UseBackgroundThreadsInCycle()) {
    TRACE_GC(heap_->tracer(), GCTracer::Scope::MC_MARK_FULL_CLOSURE_PARALLEL);
    parallel_marking_ = true;
    MarkTransitiveClosureFixpoint();
    parallel_marking_ = false;
  }

  {
    TRACE_GC(heap_->tracer(), GCTracer::Scope::MC_MARK_ROOTS);
    MarkRootsFromConservativeStack(&root_visitor);
  }

  {
    TRACE_GC(heap_->tracer(), GCTracer::Scope::MC_MARK_FULL_CLOSURE_SERIAL);
    // Complete the transitive closure single-threaded to avoid races with
    // multiple threads when processing weak maps and embedder heaps.
    CHECK(heap_->concurrent_marking()->IsStopped());
    if (auto* cpp_heap = CppHeap::From(heap_->cpp_heap())) {
      // Lock the process-global mutex here and mark cross-thread roots again.
      // This is done as late as possible to keep locking durations short.
      cpp_heap->EnterProcessGlobalAtomicPause();
    }
    if (!MarkTransitiveClosureFixpoint()) {
      MarkTransitiveClosureLinear();
    }
    CHECK(local_marking_worklists_->IsEmpty());
    CHECK(
        local_weak_objects()->current_ephemerons_local.IsLocalAndGlobalEmpty());
    CHECK(IsCppHeapMarkingFinished(heap_, local_marking_worklists_.get()));
    VerifyEphemeronMarking();
  }

  if (was_marked_incrementally) {
    // Disable the marking barrier after concurrent/parallel marking has
    // finished as it will reset page flags that share the same bitmap as
    // the evacuation candidate bit.
    MarkingBarrier::DeactivateAll(heap_);
    heap_->isolate()->traced_handles()->SetIsMarking(false);
  }

  epoch_++;
```

**File:** src/heap/mark-compact.cc (L2682-2725)
```text
class ParallelItem {
 public:
  explicit ParallelItem(const char* name, ParallelItemFunction action,
                        ParallelItemList dependencies)
      : name_(name),
        predecessors_(std::move(dependencies)),
        trace_id_(reinterpret_cast<uint64_t>(this)),
        action_(std::move(action)) {
    for (auto item : predecessors_) {
      item->add_successor(this);
    }
  }

  ParallelItem(const ParallelItem&) = delete;
  ParallelItem& operator=(const ParallelItem&) = delete;

  void Run(JobDelegate* delegate) { action_(this, delegate); }

  const ParallelItemList& successors() const { return successors_; }
  const ParallelItemList& predecessors() const { return predecessors_; }

  bool is_done() const { return is_done_; }
  void SetDone() { is_done_ = true; }

  const char* name() { return name_; }

  void add_successor(ParallelItem* item) { successors_.push_back(item); }

  bool AllPredecessorFinished() {
    ++finished_predecessors;
    return finished_predecessors == predecessors_.size();
  }

  uint64_t trace_id() const { return trace_id_; }

 private:
  const char* name_;
  ParallelItemList successors_;
  ParallelItemList predecessors_;
  size_t finished_predecessors = 0;
  const uint64_t trace_id_;
  ParallelItemFunction action_;
  bool is_done_ = false;
};
```

**File:** src/heap/mark-compact.cc (L3087-3091)
```text
void MarkCompactCollector::ClearNonLiveReferences() {
  TRACE_GC(heap_->tracer(), GCTracer::Scope::MC_CLEAR);

  auto parallel_clearing_job = std::make_unique<ParallelClearingJob>(this);
  Isolate* const isolate = heap_->isolate();
```

**File:** src/heap/mark-compact.cc (L5020-5048)
```text
size_t CreateAndExecuteEvacuationTasks(
    Heap* heap, MarkCompactCollector* collector,
    std::vector<std::pair<ParallelWorkItem, MutablePage*>> evacuation_items) {
  std::optional<ProfilingMigrationObserver> profiling_observer;
  if (heap->isolate()->log_object_relocation()) {
    profiling_observer.emplace(heap);
  }
  std::vector<std::unique_ptr<v8::internal::Evacuator>> evacuators;
  const int wanted_num_tasks = NumberOfParallelCompactionTasks(heap);
  for (int i = 0; i < wanted_num_tasks; i++) {
    auto evacuator = std::make_unique<Evacuator>(heap);
    if (profiling_observer) {
      evacuator->AddObserver(&profiling_observer.value());
    }
    evacuators.push_back(std::move(evacuator));
  }
  auto page_evacuation_job = std::make_unique<PageEvacuationJob>(
      heap->isolate(), collector, &evacuators, std::move(evacuation_items));
  TRACE_GC_NOTE_WITH_FLOW("PageEvacuationJob started",
                          page_evacuation_job->trace_id(),
                          TRACE_EVENT_FLAG_FLOW_OUT);
  V8::GetCurrentPlatform()
      ->CreateJob(v8::TaskPriority::kUserBlocking,
                  std::move(page_evacuation_job))
      ->Join();
  for (auto& evacuator : evacuators) {
    evacuator->Finalize();
  }
  return wanted_num_tasks;
```

**File:** src/heap/mark-compact.cc (L5190-5284)
```text
void MarkCompactCollector::EvacuatePagesInParallel() {
  std::vector<std::pair<ParallelWorkItem, MutablePage*>> evacuation_items;
  intptr_t live_bytes = 0;

  PinPreciseRootsIfNeeded();

  // Evacuation of new space pages cannot be aborted, so it needs to run
  // before old space evacuation.
  bool force_page_promotion =
      heap_->IsGCWithStack() && !v8_flags.compact_with_stack;
  for (NormalPage* page : new_space_evacuation_pages_) {
    intptr_t live_bytes_on_page = page->live_bytes();
    DCHECK_LT(0, live_bytes_on_page);
    live_bytes += live_bytes_on_page;
    MemoryReductionMode memory_reduction_mode =
        heap_->ShouldReduceMemory() ? MemoryReductionMode::kShouldReduceMemory
                                    : MemoryReductionMode::kNone;
    if (ShouldMovePage(page, live_bytes_on_page, memory_reduction_mode) ||
        force_page_promotion) {
      EvacuateNewToOldSpacePageVisitor::Move(page);
      DCHECK_EQ(heap_->old_space(), page->owner());
      // The move added page->allocated_bytes to the old space, but we are
      // going to sweep the page and add page->live_byte_count.
      heap_->old_space()->DecreaseAllocatedBytes(page->allocated_bytes(), page);
    }
    evacuation_items.emplace_back(ParallelWorkItem{}, page);
  }

  for (NormalPage* page : aborted_evacuation_candidates_due_to_running_code_) {
    ReportAbortedEvacuationCandidateDueToFlags(page);
  }

  if (heap_->IsGCWithStack() && !v8_flags.compact_with_stack) {
    for (NormalPage* page : old_space_evacuation_pages_) {
      ReportAbortedEvacuationCandidateDueToFlags(page);
    }
  }

  if (v8_flags.stress_compaction || v8_flags.stress_compaction_random) {
    // Stress aborting of evacuation by aborting ~5% of evacuation candidates
    // when stress testing.
    const double kFraction = 0.05;

    for (NormalPage* page : old_space_evacuation_pages_) {
      if (heap_->isolate()->fuzzer_rng()->NextDouble() < kFraction) {
        ReportAbortedEvacuationCandidateDueToFlags(page);
      }
    }
  }

  for (NormalPage* page : old_space_evacuation_pages_) {
    if (page->evacuation_was_aborted()) {
      continue;
    }

    live_bytes += page->live_bytes();
    evacuation_items.emplace_back(ParallelWorkItem{}, page);
  }

  // Promote young generation large objects.
  if (auto* new_lo_space = heap_->new_lo_space()) {
    for (auto it = new_lo_space->begin(); it != new_lo_space->end();) {
      LargePage* current = *(it++);
      Tagged<HeapObject> object = current->GetObject();
      // The black-allocated flag was already cleared in SweepLargeSpace().
      DCHECK_IMPLIES(v8_flags.black_allocated_pages,
                     !TrustedHeapLayout::InBlackAllocatedPage(object));
      if (marking_state_->IsMarked(object)) {
        heap_->lo_space()->PromoteNewLargeObject(current);
        current->set_will_be_promoted(true);
        promoted_large_pages_.push_back(current);
        evacuation_items.emplace_back(ParallelWorkItem{}, current);
      }
    }
    new_lo_space->set_objects_size(0);
  }

  const size_t pages_count = evacuation_items.size();
  size_t wanted_num_tasks = 0;
  if (!evacuation_items.empty()) {
    TRACE_EVENT1(TRACE_DISABLED_BY_DEFAULT("v8.gc"),
                 "MarkCompactCollector::EvacuatePagesInParallel", "pages",
                 evacuation_items.size());

    wanted_num_tasks = CreateAndExecuteEvacuationTasks(
        heap_, this, std::move(evacuation_items));
  }

  const size_t aborted_pages = PostProcessAbortedEvacuationCandidates();

  if (V8_UNLIKELY(v8_flags.trace_evacuation)) {
    TraceEvacuation(heap_->isolate(), pages_count, wanted_num_tasks, live_bytes,
                    aborted_pages);
  }
}
```

**File:** src/heap/mark-compact.cc (L6319-6386)
```text
void MarkCompactCollector::Sweep() {
  DCHECK(!sweeper_->sweeping_in_progress());

  sweeper_->InitializeMajorSweeping();

  TRACE_GC_EPOCH_WITH_FLOW(
      heap_->tracer(), GCTracer::Scope::MC_SWEEP, ThreadKind::kMain,
      sweeper_->GetTraceIdForFlowEvent(GCTracer::Scope::MC_SWEEP),
      TRACE_EVENT_FLAG_FLOW_OUT);
#ifdef DEBUG
  state_ = SWEEP_SPACES;
#endif

  {
    GCTracer::Scope sweep_scope(heap_->tracer(), GCTracer::Scope::MC_SWEEP_LO,
                                ThreadKind::kMain);
    SweepLargeSpace(heap_->lo_space());
  }
  {
    GCTracer::Scope sweep_scope(
        heap_->tracer(), GCTracer::Scope::MC_SWEEP_CODE_LO, ThreadKind::kMain);
    SweepLargeSpace(heap_->code_lo_space());
  }
  if (heap_->shared_space()) {
    GCTracer::Scope sweep_scope(heap_->tracer(),
                                GCTracer::Scope::MC_SWEEP_SHARED_LO,
                                ThreadKind::kMain);
    SweepLargeSpace(heap_->shared_lo_space());
  }
  {
    GCTracer::Scope sweep_scope(heap_->tracer(), GCTracer::Scope::MC_SWEEP_OLD,
                                ThreadKind::kMain);
    StartSweepSpace(heap_->old_space());
  }
  {
    GCTracer::Scope sweep_scope(heap_->tracer(), GCTracer::Scope::MC_SWEEP_CODE,
                                ThreadKind::kMain);
    StartSweepSpace(heap_->code_space());
  }
  if (heap_->shared_space()) {
    GCTracer::Scope sweep_scope(
        heap_->tracer(), GCTracer::Scope::MC_SWEEP_SHARED, ThreadKind::kMain);
    StartSweepSpace(heap_->shared_space());
  }
  {
    GCTracer::Scope sweep_scope(
        heap_->tracer(), GCTracer::Scope::MC_SWEEP_TRUSTED, ThreadKind::kMain);
    StartSweepSpace(heap_->trusted_space());
  }
  if (heap_->shared_trusted_space()) {
    GCTracer::Scope sweep_scope(
        heap_->tracer(), GCTracer::Scope::MC_SWEEP_SHARED, ThreadKind::kMain);
    StartSweepSpace(heap_->shared_trusted_space());
  }
  {
    GCTracer::Scope sweep_scope(heap_->tracer(),
                                GCTracer::Scope::MC_SWEEP_TRUSTED_LO,
                                ThreadKind::kMain);
    SweepLargeSpace(heap_->trusted_lo_space());
  }
  if (v8_flags.minor_ms && heap_->new_space()) {
    GCTracer::Scope sweep_scope(heap_->tracer(), GCTracer::Scope::MC_SWEEP_NEW,
                                ThreadKind::kMain);
    StartSweepNewSpace();
  }

  sweeper_->StartMajorSweeping();
}
```

**File:** src/heap/sweeper.h (L199-203)
```text
  class ConcurrentMajorSweeper;
  class ConcurrentMinorSweeper;

  class MajorSweeperJob;
  class MinorSweeperJob;
```

**File:** src/flags/flag-definitions.h (L2496-2550)
```text
DEFINE_BOOL(incremental_marking, true, "use incremental marking")
DEFINE_BOOL(incremental_marking_task, true, "use tasks for incremental marking")
DEFINE_INT(incremental_marking_soft_trigger, 0,
           "threshold for starting incremental marking via a task in percent "
           "of available space: limit - size")
DEFINE_INT(incremental_marking_hard_trigger, 0,
           "threshold for starting incremental marking immediately in percent "
           "of available space: limit - size")
DEFINE_BOOL(incremental_marking_unified_schedule, false,
            "Use a single schedule for determining a marking schedule between "
            "JS and C++ objects.")
DEFINE_DEVELOPER_FLAG(trace_unmapper, "Trace the unmapping")
DEFINE_BOOL(parallel_scavenge, true, "parallel scavenge")
DEFINE_BOOL(minor_gc_task, true, "schedule minor GC tasks")
DEFINE_UINT(minor_gc_task_trigger, 80,
            "minor GC task trigger in percent of the current heap limit")
DEFINE_BOOL(minor_gc_task_with_lower_priority, true,
            "schedules the minor GC task with kUserVisible priority.")
DEFINE_BOOL(trace_parallel_scavenge, false, "trace parallel scavenge")
DEFINE_EXPERIMENTAL_FEATURE(
    cppgc_young_generation,
    "run young generation garbage collections in Oilpan")
// CppGC young generation (enables unified young heap) is based on Minor MS.
DEFINE_IMPLICATION(cppgc_young_generation, minor_ms)
// Unified young generation disables the unmodified wrapper reclamation
// optimization.
DEFINE_NEG_IMPLICATION(cppgc_young_generation, reclaim_unmodified_wrappers)
DEFINE_BOOL(optimize_gc_for_battery, false, "optimize GC for battery")
#if defined(V8_ATOMIC_OBJECT_FIELD_WRITES)
DEFINE_BOOL(concurrent_marking, true, "use concurrent marking")
#else
// Concurrent marking cannot be used without atomic object field loads and
// stores.
DEFINE_BOOL(concurrent_marking, false, "use concurrent marking")
#endif
DEFINE_INT(
    concurrent_marking_max_worker_num, 7,
    "max worker number of concurrent marking, 0 for NumberOfWorkerThreads")
DEFINE_BOOL(concurrent_array_buffer_sweeping, true,
            "concurrently sweep array buffers")
DEFINE_BOOL(stress_concurrent_allocation, false,
            "start background threads that allocate memory")
DEFINE_BOOL(parallel_marking, true, "use parallel marking in atomic pause")
DEFINE_INT(ephemeron_fixpoint_iterations, 10,
           "number of fixpoint iterations it takes to switch to linear "
           "ephemeron algorithm")
DEFINE_BOOL(trace_concurrent_marking, false, "trace concurrent marking")
DEFINE_BOOL(concurrent_sweeping, true, "use concurrent sweeping")
DEFINE_NEG_NEG_IMPLICATION(concurrent_sweeping,
                           concurrent_array_buffer_sweeping)
DEFINE_BOOL(parallel_compaction, true, "use parallel compaction")
DEFINE_BOOL(parallel_pointer_update, true,
            "use parallel pointer update during compaction")
DEFINE_BOOL(parallel_gc_clearing, true,
            "use parallel threads to clear weak refs in the atomic pause.")
```

**File:** src/flags/flag-definitions.h (L4108-4121)
```text
//
// Parallel and concurrent GC (Orinoco) related flags.
//
DEFINE_BOOL(single_threaded_gc, false, "disable the use of background gc tasks")
DEFINE_NEG_IMPLICATION(single_threaded_gc, concurrent_marking)
DEFINE_NEG_IMPLICATION(single_threaded_gc, concurrent_sweeping)
DEFINE_NEG_IMPLICATION(single_threaded_gc, parallel_compaction)
DEFINE_NEG_IMPLICATION(single_threaded_gc, parallel_marking)
DEFINE_NEG_IMPLICATION(single_threaded_gc, parallel_pointer_update)
DEFINE_NEG_IMPLICATION(single_threaded_gc, parallel_gc_clearing)
DEFINE_NEG_IMPLICATION(single_threaded_gc, parallel_scavenge)
DEFINE_NEG_IMPLICATION(single_threaded_gc, concurrent_array_buffer_sweeping)
DEFINE_NEG_IMPLICATION(single_threaded_gc, stress_concurrent_allocation)
DEFINE_NEG_IMPLICATION(single_threaded_gc, cppheap_concurrent_marking)
```

**File:** src/flags/flag-definitions.h (L4123-4128)
```text
DEFINE_BOOL(single_threaded_gc_in_background, true,
            "disable the use of background gc tasks when in background")
DEFINE_BOOL(parallel_pause_for_gc_in_background, true,
            "Use parallel threads in the atomic pause for background GCs")
DEFINE_BOOL(incremental_marking_for_gc_in_background, true,
            "Use parallel threads in the atomic pause for background GCs")
```

**File:** src/heap/scavenger.cc (L778-789)
```text
size_t ScavengerJobTask::GetMaxConcurrency(size_t worker_count) const {
  // We need to account for local segments held by worker_count in addition to
  // GlobalPoolSize() of copied_list_, pinned_list_ and promoted_list_.
  size_t wanted_num_workers = std::max<size_t>(
      remaining_memory_chunks_.load(std::memory_order_relaxed),
      worker_count + copied_list_.Size() + promoted_list_.Size());
  if (!heap_->ShouldUseBackgroundThreads() ||
      heap_->ShouldOptimizeForBattery()) {
    return std::min<size_t>(wanted_num_workers, 1);
  }
  return std::min<size_t>(scavengers_->size(), wanted_num_workers);
}
```

**File:** src/heap/scavenger.cc (L1722-1771)
```text
  {
    // Start the parallel scavenger job before iterating roots. This allows
    // background threads to start processing old_to_new pages while the main
    // thread iterates roots in parallel.
    TRACE_GC_ARG1(heap_->tracer(),
                  GCTracer::Scope::SCAVENGER_SCAVENGE_PARALLEL_PHASE,
                  "UseBackgroundThreads", heap_->ShouldUseBackgroundThreads());
    std::atomic<size_t> estimate_concurrency{0};
    auto job = std::make_unique<ScavengerJobTask>(
        heap_, &scavengers, std::move(old_to_new_chunks), copied_list,
        promoted_list, estimate_concurrency);
    TRACE_GC_NOTE_WITH_FLOW("Parallel scavenge started", job->trace_id(),
                            TRACE_EVENT_FLAG_FLOW_OUT);
    std::unique_ptr<JobHandle> job_handle = V8::GetCurrentPlatform()->PostJob(
        v8::TaskPriority::kUserBlocking, std::move(job));

    // Iterate roots on the main thread while background threads scavenge pages.
    {
      // Copy roots.
      TRACE_GC(heap_->tracer(), GCTracer::Scope::SCAVENGER_SCAVENGE_ROOTS);
      RootScavengeVisitor root_scavenge_visitor(main_thread_scavenger);

      // Scavenger treats all weak roots except for global handles as strong.
      // That is why we don't set skip_weak = true here and instead visit
      // global handles separately.
      base::EnumSet<SkipRoot> options(
          {SkipRoot::kExternalStringTable, SkipRoot::kGlobalHandles,
           SkipRoot::kTracedHandles, SkipRoot::kOldGeneration,
           SkipRoot::kConservativeStack, SkipRoot::kReadOnlyBuiltins});
      if (is_using_precise_pinning) {
        options.Add({SkipRoot::kMainThreadHandles, SkipRoot::kStack});
      }

      heap_->IterateRoots(&root_scavenge_visitor, options);
      isolate->global_handles()->IterateYoungStrongAndDependentRoots(
          &root_scavenge_visitor);
      isolate->traced_handles()->IterateYoungRoots(&root_scavenge_visitor);
    }
    // The destructor of RootScavengeVisitor calls Publish(), which publishes
    // the main thread scavenger's local copied and promoted lists to the global
    // worklists, making them available for processing by any worker thread.
    // main_thread_scavenger is not used from this point forward anymore.

    // Notify the job system that more work is available now that root scanning
    // is finished.
    job_handle->NotifyConcurrencyIncrease();

    // Join the parallel job: participate in remaining work and wait for
    // completion.
    job_handle->Join();
```

**File:** src/heap/heap.cc (L602-614)
```text
bool Heap::ShouldUseBackgroundThreads() const {
  return !v8_flags.single_threaded_gc_in_background ||
         !isolate()->EfficiencyModeEnabled();
}

bool Heap::ShouldUseIncrementalMarking() const {
  if (v8_flags.single_threaded_gc_in_background &&
      isolate()->EfficiencyModeEnabled()) {
    return v8_flags.incremental_marking_for_gc_in_background;
  } else {
    return true;
  }
}
```

**File:** src/heap/heap.cc (L1295-1322)
```text
void Heap::StartMinorMSConcurrentMarkingIfNeeded() {
  if (incremental_marking()->IsMarking()) return;
  if (v8_flags.concurrent_minor_ms_marking && !IsTearingDown() &&
      incremental_marking()->CanAndShouldBeStarted() &&
      V8_LIKELY(!v8_flags.gc_global)) {
    size_t usable_capacity = 0;
    size_t new_space_size = 0;
    if (v8_flags.sticky_mark_bits) {
      // TODO(333906585): Adjust parameters.
      usable_capacity =
          sticky_space()->Capacity() - sticky_space()->old_objects_size();
      new_space_size = sticky_space()->young_objects_size();
    } else {
      usable_capacity = paged_new_space()->paged_space()->UsableCapacity();
      new_space_size = new_space()->Size();
    }
    if ((usable_capacity >=
         v8_flags.minor_ms_min_new_space_capacity_for_concurrent_marking_mb *
             MB) &&
        (new_space_size >= MinorMSConcurrentMarkingTrigger(this)) &&
        ShouldUseBackgroundThreads()) {
      StartIncrementalMarking(GCFlag::kNoFlags, GarbageCollectionReason::kTask,
                              kNoGCCallbackFlags,
                              GarbageCollector::MINOR_MARK_SWEEPER);
      // Schedule a task for finalizing the GC if needed.
      minor_gc_job()->TryScheduleTask();
    }
  }
```

**File:** src/heap/minor-mark-sweep.cc (L695-712)
```text
void MinorMarkSweepCollector::MarkLiveObjects() {
  TRACE_GC(heap_->tracer(), GCTracer::Scope::MINOR_MS_MARK);

  const bool was_marked_incrementally =
      !heap_->incremental_marking()->IsStopped();
  if (!was_marked_incrementally) {
    StartMarking(false);
  } else {
    auto* incremental_marking = heap_->incremental_marking();
    TRACE_GC_WITH_FLOW(
        heap_->tracer(), GCTracer::Scope::MINOR_MS_MARK_FINISH_INCREMENTAL,
        incremental_marking->current_trace_id(), TRACE_EVENT_FLAG_FLOW_IN);
    DCHECK(incremental_marking->IsMinorMarking());
    DCHECK(v8_flags.concurrent_minor_ms_marking);
    incremental_marking->Stop();
    MarkingBarrier::PublishYoung(heap_);
  }

```

**File:** src/heap/marking-barrier.h (L26-117)
```text
class MarkingBarrier {
 public:
  explicit MarkingBarrier(LocalHeap*);
  ~MarkingBarrier();

  void Activate(bool is_compacting, MarkingMode marking_mode);
  void Deactivate();
  void PublishIfNeeded();

  void ActivateShared();
  void DeactivateShared();
  void PublishSharedIfNeeded();

  static void ActivateAll(Heap* heap, bool is_compacting);
  static void DeactivateAll(Heap* heap);
  V8_EXPORT_PRIVATE static void PublishAll(Heap* heap);

  static void ActivateYoung(Heap* heap);
  static void DeactivateYoung(Heap* heap);
  V8_EXPORT_PRIVATE static void PublishYoung(Heap* heap);

  template <typename TSlot, RecordYoungSlot kRecordYoung = RecordYoungSlot::kNo>
  void Write(Tagged<HeapObject> host, TSlot slot, Tagged<HeapObject> value);
  void Write(Tagged<HeapObject> host, IndirectPointerSlot slot);
  void Write(Tagged<InstructionStream> host, RelocInfo*,
             Tagged<HeapObject> value);
  void Write(Tagged<JSArrayBuffer> host, ArrayBufferExtension*);
  void Write(Tagged<DescriptorArray>, int number_of_own_descriptors);
  // Only usable when there's no valid JS host object for this write, e.g., when
  // value is held alive from a global handle.
  void WriteWithoutHost(Tagged<HeapObject> value);

  inline void MarkValue(Tagged<HeapObject> host, Tagged<HeapObject> value);

  bool is_minor() const { return marking_mode_ == MarkingMode::kMinorMarking; }

  bool is_not_major() const {
    switch (marking_mode_) {
      case MarkingMode::kMajorMarking:
        return false;
      case MarkingMode::kNoMarking:
      case MarkingMode::kMinorMarking:
        return true;
    }
  }

  Heap* heap() const { return heap_; }

#if DEBUG
  void AssertMarkingIsActivated() const;
  void AssertSharedMarkingIsActivated() const;
#endif  // DEBUG

#if V8_VERIFY_WRITE_BARRIERS
  bool IsMarked(const Tagged<HeapObject> value) const;
#endif  // V8_VERIFY_WRITE_BARRIERS

 private:
  inline void MarkValueShared(Tagged<HeapObject> value);
  inline void MarkValueLocal(Tagged<HeapObject> value);

  void RecordRelocSlot(Tagged<InstructionStream> host, RelocInfo* rinfo,
                       Tagged<HeapObject> target);

  bool IsCurrentMarkingBarrier(Tagged<HeapObject> verification_candidate);

  template <typename TSlot>
  inline void MarkRange(Tagged<HeapObject> value, TSlot start, TSlot end);

  inline bool IsCompacting(Tagged<HeapObject> object) const;

  bool is_major() const { return marking_mode_ == MarkingMode::kMajorMarking; }

  Isolate* isolate() const;

  Heap* heap_;
  MarkCompactCollector* major_collector_;
  MinorMarkSweepCollector* minor_collector_;
  IncrementalMarking* incremental_marking_;
  std::unique_ptr<MarkingWorklists::Local> current_worklists_;
  std::optional<MarkingWorklists::Local> shared_heap_worklists_;
  MarkingState marking_state_;
  std::unordered_map<MutablePage*, std::unique_ptr<TypedSlots>,
                     base::hash<MutablePage*>>
      typed_slots_map_;
  bool is_compacting_ = false;
  bool is_activated_ = false;
  const bool is_main_thread_barrier_;
  const bool uses_shared_heap_;
  const bool is_shared_space_isolate_;
  MarkingMode marking_mode_ = MarkingMode::kNoMarking;
};
```

**File:** src/heap/WRITE_BARRIER.md (L1-11)
```markdown
# V8 Write Barrier - Readme

V8 uses a write barrier to inform the GC about changes to the heap by the mutator.
A write barrier is emitted for heap stores like `host.field = value`.
The write barrier is required for multiple purposes:
* Records old-to-new references for the generational GC to work.
* During marking it prevents black-to-white references during incremental/concurrent marking.
* During marking it records old-to-old references (pointers to objects on evacuation candidates)

The generational barrier is always enabled, while the other barriers are only enabled while incremental/concurrent marking is running.

```

**File:** src/heap/incremental-marking.h (L1-4)
```text
// Copyright 2012 the V8 project authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

```

**File:** src/heap/concurrent-marking.h (L1-4)
```text
// Copyright 2017 the V8 project authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

```

**File:** src/heap/minor-mark-sweep.h (L1-4)
```text
// Copyright 2023 the V8 project authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

```
