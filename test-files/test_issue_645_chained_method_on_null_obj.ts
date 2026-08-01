// Closes #645 — chained method calls on a value that fell through the old
// `js_native_call_method` catch-all used to turn a sentinel into numeric zero
// and fail unpredictably later in the chain. Since #648 unknown methods on real
// objects deliberately throw immediately; the stored expected output asserts
// that this path remains an ordinary catchable TypeError rather than a signal
// death or numeric-pointer crash.
//
// Repro shape (drizzle's `this.stmt.raw().all(...params)` boiled down
// to its essentials): a chained method call where the receiver of
// each step is the result of the previous step. With Perry's existing
// "fall through to NULL_OBJECT_BYTES stub" semantics for unknown
// methods, every step in the chain must produce a `typeof === "object"`
// value — not a number — so the chain doesn't crash mid-way.
//
// Acceptance: the program exits with the pinned TypeError at the first unknown
// call. Node also throws there, but its source-located diagnostic differs.

const obj: any = {};
const r1 = obj.nonExistentMethodA();
const r2 = r1.nonExistentMethodB();
const r3 = obj.a().b().c();

// Print a sentinel so a successful run is visibly distinguishable from
// the pre-fix crash (the crash printed nothing to stdout before exit).
console.log("ok r1=", typeof r1, "r2=", typeof r2, "r3=", typeof r3);
