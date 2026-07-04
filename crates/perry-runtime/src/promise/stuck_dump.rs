//! #5941 diagnostic toolkit — perturbation-proof stuck-async-step-machine
//! dumper. Fully inert unless the process runs with `PERRY_STUCK_DUMP=1`
//! (one relaxed atomic load per hook call otherwise). DIAGNOSTIC ONLY:
//! strip before shipping a PR.
//!
//! Design constraints (learned across the #5437/#5941 sessions):
//!  - JS-level or env-gated *behavioral* probes perturb the deadlock;
//!    this toolkit only RECORDS (global mutex map) and prints from a
//!    background thread, so timing on the main thread is unchanged.
//!  - The background dumper must not dereference heap values (they live
//!    in the per-thread arena) — awaited values are classified by
//!    NaN-tag bits only, and graph edges are computed by pointer
//!    equality against recorded machine-result pointers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

static ENABLED: AtomicU8 = AtomicU8::new(2); // 2 = unresolved, 1 = on, 0 = off

#[inline]
pub(crate) fn enabled() -> bool {
    match ENABLED.load(Ordering::Relaxed) {
        0 => false,
        1 => true,
        _ => {
            let on = std::env::var("PERRY_STUCK_DUMP").is_ok_and(|v| v == "1");
            ENABLED.store(u8::from(on), Ordering::Relaxed);
            if on {
                start_dumper();
            }
            on
        }
    }
}

#[derive(Clone, Default)]
struct Machine {
    /// NaN-boxed bits of the value most recently passed to
    /// `js_async_step_chain` for this step (the awaited value).
    last_await_bits: u64,
    /// The machine's result promise as returned to its first caller
    /// (from `js_async_first_call`) — what outer awaiters wait on.
    result_promise: usize,
    /// Every continuation promise this machine has minted/reused across
    /// its awaits (chain `next`, backpatched thunk results). An awaiter
    /// parked on any of these is awaiting THIS machine, not an external
    /// promise. Capped; the steady state reuses one promise.
    owned: Vec<usize>,
    /// The promise `js_async_step_done` settled (reuse path) or minted
    /// (non-reuse path). Non-zero implies the machine finished.
    done_promise: usize,
    chains: u32,
    thunk_fires: u32,
    first_calls: u32,
}

static MACHINES: Mutex<Option<HashMap<usize, Machine>>> = Mutex::new(None);

/// promise → executor closure func_ptr, for every `new Promise(executor)`
/// (`js_promise_new_with_executor`). Lets the dumper name the CREATION
/// SITE of an EXTERNAL awaited promise via atos on a
/// PERRY_DEBUG_SYMBOLS build.
static EXEC_PROMISES: Mutex<Option<HashMap<usize, usize>>> = Mutex::new(None);

pub(crate) fn note_executor_promise(promise: usize, executor_func_ptr: usize) {
    if promise == 0 {
        return;
    }
    let mut guard = EXEC_PROMISES.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .get_or_insert_with(HashMap::new)
        .insert(promise, executor_func_ptr);
}

fn executor_of(promise: usize) -> Option<usize> {
    let guard = EXEC_PROMISES.lock().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().and_then(|m| m.get(&promise).copied())
}

/// child promise → (parent promise, creation-site tag) for every
/// `js_promise_new_with_parent` (the `.then`/`.finally`/await chain
/// intermediary constructor). Lets the dumper walk an EXTERNAL awaited
/// promise up its chain to the true root.
static PARENTS: Mutex<Option<HashMap<usize, usize>>> = Mutex::new(None);

pub(crate) fn note_parent(child: usize, parent: usize) {
    if child == 0 {
        return;
    }
    let mut guard = PARENTS.lock().unwrap_or_else(|e| e.into_inner());
    guard.get_or_insert_with(HashMap::new).insert(child, parent);
}

fn parent_of(promise: usize) -> Option<usize> {
    let guard = PARENTS.lock().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().and_then(|m| m.get(&promise).copied())
}

/// Describe a promise for the dump: machine-owned / executor-created /
/// then-chain child (walking up to 8 ancestors) / untracked root.
fn describe_promise(
    p: usize,
    result_owner: &HashMap<usize, (usize, bool)>,
) -> String {
    let mut out = String::new();
    let mut cur = p;
    for hop in 0..8 {
        if hop > 0 {
            out.push_str(" <- ");
        }
        if let Some((owner, done)) = result_owner.get(&cur) {
            out.push_str(&format!(
                "{:#x}[machine {:#x}{}]",
                cur,
                owner,
                if *done { " DONE" } else { " STUCK" }
            ));
            // A DONE machine's result that carries an adoption/parent edge
            // is itself waiting on that parent (`return <promise>`) — keep
            // walking to find what actually never settles.
            if *done {
                if let Some(parent) = parent_of(cur) {
                    if parent != 0 && parent != cur {
                        cur = parent;
                        continue;
                    }
                }
            }
            return out;
        }
        if let Some(exec_fn) = executor_of(cur) {
            out.push_str(&format!("{:#x}[executor fn={:#x}]", cur, exec_fn));
            return out;
        }
        match parent_of(cur) {
            Some(0) => {
                out.push_str(&format!("{:#x}[then-child of NULL-parent]", cur));
                return out;
            }
            Some(parent) => {
                out.push_str(&format!("{:#x}[then-child]", cur));
                cur = parent;
            }
            None => {
                out.push_str(&format!("{:#x}[UNTRACKED root]", cur));
                return out;
            }
        }
    }
    out.push_str("...");
    out
}

fn with_machines<R>(f: impl FnOnce(&mut HashMap<usize, Machine>) -> R) -> R {
    let mut guard = MACHINES.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// A step closure is entering `js_async_first_call`. If the same pointer
/// is already tracked as a LIVE (not-done, has-awaited) activation, two
/// activations share one step closure — the #5941 identity-collision
/// smoking gun. (Caveat: a false positive is possible if a dead-but-
/// never-done machine's closure was GC-recycled into a new closure at
/// the same address; treat repeated hits as the signal, not one.)
pub(crate) fn note_first_call(step: usize) {
    with_machines(|m| {
        let entry = m.entry(step).or_default();
        if entry.done_promise == 0 && entry.chains > 0 {
            eprintln!(
                "[COLLIDE] step {:#x} re-entered first_call while LIVE (chains={} thunks={} first_calls={} last_await={:#x})",
                step, entry.chains, entry.thunk_fires, entry.first_calls, entry.last_await_bits
            );
        }
        if entry.done_promise != 0 {
            // Completed machine re-invoked (or address recycled): fresh activation.
            *entry = Machine::default();
        }
        entry.first_calls += 1;
    });
}

/// Record the result promise the first caller receives (NaN-boxed bits).
pub(crate) fn note_first_call_result(step: usize, result_bits: u64) {
    let ptr = decode_pointer(result_bits);
    if ptr != 0 {
        with_machines(|m| {
            m.entry(step).or_default().result_promise = ptr;
        });
    }
}

/// `js_async_step_chain` ran for `step` with awaited value `value_bits`,
/// yielding `next` as the machine's continuation promise.
pub(crate) fn note_chain(step: usize, value_bits: u64, next: usize) {
    with_machines(|m| {
        let entry = m.entry(step).or_default();
        entry.chains += 1;
        entry.last_await_bits = value_bits;
        if next != 0 && !entry.owned.contains(&next) && entry.owned.len() < 16 {
            entry.owned.push(next);
        }
    });
}

/// `js_async_step_done` ran for `step`, settling/minting `done_promise`.
pub(crate) fn note_done(step: usize, done_promise: usize) {
    with_machines(|m| {
        let entry = m.entry(step).or_default();
        entry.done_promise = if done_promise == 0 { 1 } else { done_promise };
    });
}

/// A promise was created/registered on behalf of `step`'s activation
/// (e.g. the backpatched result on the pending-await thunk path).
pub(crate) fn note_owned(step: usize, promise: usize) {
    if step == 0 || promise == 0 {
        return;
    }
    with_machines(|m| {
        let entry = m.entry(step).or_default();
        if !entry.owned.contains(&promise) && entry.owned.len() < 16 {
            entry.owned.push(promise);
        }
    });
}

/// A resume thunk fired for `step`.
pub(crate) fn note_thunk_fire(step: usize) {
    with_machines(|m| {
        m.entry(step).or_default().thunk_fires += 1;
    });
}

fn decode_pointer(bits: u64) -> usize {
    const TAG_MASK: u64 = 0xFFFF_0000_0000_0000;
    const POINTER_TAG: u64 = 0x7FFD_0000_0000_0000;
    const PTR_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
    if bits & TAG_MASK == POINTER_TAG {
        (bits & PTR_MASK) as usize
    } else {
        0
    }
}

fn classify(bits: u64) -> &'static str {
    const TAG_MASK: u64 = 0xFFFF_0000_0000_0000;
    match bits & TAG_MASK {
        0x7FFD_0000_0000_0000 => "ptr/promise?",
        0x7FFF_0000_0000_0000 => "string",
        0x7FFE_0000_0000_0000 => "int32",
        0x7FFA_0000_0000_0000 => "bigint",
        0x7FFC_0000_0000_0000 => match bits {
            0x7FFC_0000_0000_0001 => "undefined",
            0x7FFC_0000_0000_0002 => "null",
            0x7FFC_0000_0000_0003 => "false",
            0x7FFC_0000_0000_0004 => "true",
            _ => "special",
        },
        _ => "number",
    }
}

fn start_dumper() {
    std::thread::Builder::new()
        .name("perry-stuck-dump".into())
        .spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(6));
            dump();
        })
        .ok();
}

fn dump() {
    with_machines(|m| {
        let live: Vec<(usize, Machine)> = m
            .iter()
            .filter(|(_, st)| st.done_promise == 0 && st.chains > 0)
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        if live.is_empty() {
            return;
        }
        // Await-graph: awaited pointer vs every promise any machine has
        // owned (first_call result, chain nexts, done settlement). LIVE
        // owners are inserted last so a live owner wins over a done
        // machine that merely adopted the same promise.
        let mut result_owner: HashMap<usize, (usize, bool)> = HashMap::new();
        for (step, st) in m.iter().filter(|(_, st)| st.done_promise != 0) {
            for p in std::iter::once(st.result_promise)
                .chain(st.owned.iter().copied())
                .chain(std::iter::once(st.done_promise))
            {
                if p > 1 {
                    result_owner.insert(p, (*step, true));
                }
            }
        }
        for (step, st) in m.iter().filter(|(_, st)| st.done_promise == 0) {
            for p in std::iter::once(st.result_promise).chain(st.owned.iter().copied()) {
                if p > 1 {
                    result_owner.insert(p, (*step, false));
                }
            }
        }
        eprintln!("[STUCK] ---- {} live async-step machines ----", live.len());
        // Inventory of every DONE machine whose promises appear in a live
        // walk — the done-orphan check: done_promise != result_promise
        // means `js_async_step_done` settled a promise nobody holds.
        let mut printed_owners: Vec<usize> = Vec::new();
        for (_, st) in live.iter() {
            let mut cur = decode_pointer(st.last_await_bits);
            for _ in 0..8 {
                if cur == 0 {
                    break;
                }
                if let Some((owner, true)) = result_owner.get(&cur) {
                    if !printed_owners.contains(owner) {
                        printed_owners.push(*owner);
                        if let Some(o) = m.get(owner) {
                            let func_ptr = unsafe { *(*owner as *const *const u8) } as usize;
                            eprintln!(
                                "[DONE-OWNER] step={:#x} fn={:#x} chains={} thunks={} fc={} result={:#x} done_promise={:#x} owned={:x?}",
                                owner, func_ptr, o.chains, o.thunk_fires, o.first_calls,
                                o.result_promise, o.done_promise, o.owned
                            );
                        }
                    }
                    break;
                }
                match parent_of(cur) {
                    Some(p) if p != 0 => cur = p,
                    _ => break,
                }
            }
        }
        for (step, st) in live.iter().take(48) {
            let awaited = decode_pointer(st.last_await_bits);
            let link = if awaited == 0 {
                "non-ptr".to_string()
            } else {
                format!("awaits {}", describe_promise(awaited, &result_owner))
            };
            // Diagnostic-only cross-thread read of the closure header's
            // func_ptr so `atos` can name the stuck JS function on a
            // PERRY_DEBUG_SYMBOLS build. Closures never carry a null
            // func_ptr; the header is alive while the machine is live.
            let func_ptr = unsafe { *(*step as *const *const u8) } as usize;
            eprintln!(
                "[STUCK] step={:#x} fn={:#x} chains={} thunks={} fc={} await={:#x}({}) result={:#x} -> {}",
                step,
                func_ptr,
                st.chains,
                st.thunk_fires,
                st.first_calls,
                st.last_await_bits,
                classify(st.last_await_bits),
                st.result_promise,
                link
            );
        }
    });
}
