### Fixed

- **GC: a budgeted incremental cycle that nothing drives now completes (#6978).**
  The budgeted stepper had a budget but no completion guarantee. It is driven
  by exactly two things — allocation-point mutator assists (`gc_check_trigger`)
  and host safepoints (`gc_runtime_safepoint`, from the microtask checkpoint
  and the stdlib pump) — so a program that stops allocating stopped driving it
  and the cycle parked for the life of the process.

  Measured on `test_gap_repsel_canonical_i32` under `PERRY_GC_HEAP_LIMIT=8`
  (release, macOS arm64): `gc_check_trigger` runs **twice in the whole
  process**. The trigger is `ArenaBytes` — this program never calls
  `gc_malloc` at all (`malloc_count=0` against a 100 000 trigger), so the
  malloc-trigger mechanism #6978 hypothesised is not involved. The second call
  arms the cycle and pays one 256-unit assist, and no allocation-point
  opportunity ever comes again; the host safepoints that follow each pay the
  fixed 2 048-unit slice and get the cycle through `BuildValidPointerSet` and
  one step into `RootScan` before the process exits. The assist *budget* was
  never the binding constraint — the number of opportunities was, and a cycle
  has seven resumable phases with one `step()` advancing at most one of them,
  so completion needs a bounded number of calls rather than more units.

  A parked cycle is not inert: nothing is reclaimed, the arming trigger is
  never re-baselined, every subsequent allocation is born **black** so the
  parked cycle can never collect it, the incremental mark barrier stays armed
  on every store, and `gc_safepoint_moving_minor` (the precise-root copying
  minor at the outermost microtask boundary) early-returns on
  `gc_budgeted_cycle_active()` — so the parked cycle also disabled the
  collector's own moving path. Net effect on a compiled program in the shipped
  configuration: zero completed collections.

  The host safepoint now carries the guarantee — the one point in the process
  where the mutator has yielded and the JS stack has unwound. A cycle may span
  `GC_CYCLE_HOST_SAFEPOINT_LIMIT` (2) safepoints on the ordinary bounded
  budget; at the next one the safepoint drives it to completion, in a loop
  bounded exactly like `gc_drain_active_budgeted_cycle`. Kill switch
  `PERRY_GC_SAFEPOINT_FINISH=0`.

  The limit does not bind on healthy workloads, measured: on
  `test_gap_repsel_gc_stress` the budgeted cycles complete on mutator assists
  alone (20 / 25 / 35 assist steps, **zero** host safepoints), and an async
  probe that yields every 256 iterations while allocating reports
  `safepoint_steps=0` in every configuration.

  `PERRY_GC_TRACE=1` cycle counts under `PERRY_GC_HEAP_LIMIT=8` with no other
  GC env var: `test_gap_repsel_canonical_i32` 0 → 1,
  `test_gap_repsel_ptr_shape_locals` 0 → 1, `test_gap_repsel_gc_stress`
  21 → 22, all still byte-exact against node 26.5.0.
  `scripts/gc_repsel_matrix.sh --arms all --pressure 8` (440 cells, same
  binaries A/B'd through the kill switch): `PASS=229 UNVER=190 XFAIL=1
  FAIL=20` → `PASS=400 UNVER=19 XFAIL=1 FAIL=20`, the only cell transition
  being `UNVER → PASS` × 171, with an identical FAIL set (#6981) and no
  `PASS → non-PASS`. Costs one collection's worth of work that previously
  never ran (+1.2 – 2.7 % wall clock where a heap budget makes a trigger due,
  0 % without one) and does **not** regress max pause: the added cycle's
  largest step is 16.0 ms against the run's pre-existing 54.6 ms maximum.
