// GC ratchet probe: collect WHILE the stack is deep, with a live root in
// every frame.
//
// Every other probe in this suite calls `gc()` at the end, from a shallow
// stack, so the native-root walker has almost nothing to walk. Measured on the
// same suite: `04_dead_after_deep_stack` reports 7 frames visited and **zero**
// root locations on macOS and Linux — those arms would pass unchanged with a
// walker that visited nothing at all. The Windows arm only exercises a deep
// walk (5,626 frames, 5,449 records) by accident of heap sizing, not by design.
//
// This probe makes that coverage deliberate and portable. `descend` holds a
// heap value live ACROSS its recursive call, and the collection happens at the
// deepest point — so at collection time there is one live root per frame, all
// of them mid-frame rather than in the leaf. A walker that stops early, or a
// map whose bases are wrong, loses roots that are still read afterwards, and
// the checksum diverges from the oracle instead of the run merely being slower.
//
// Under `PERRY_GC_FORCE_EVACUATE=1` every survivor also MOVES, so a stale
// pointer is a wrong value rather than a lucky one.

declare function gc(): void;

const DEPTH = 220;
const ROUNDS = 3;

class Payload {
  tag: number;
  body: string;
  constructor(tag: number) {
    this.tag = tag;
    this.body = "d" + tag;
  }
  value(): number {
    return (this.tag + this.body.length) | 0;
  }
}

let escape: Payload | null = null;

function descend(depth: number): number {
  // Live across the recursive call below, which is where the collection
  // happens. Its contents are read after that call returns, so the collector
  // must have found and relocated this slot.
  const mine = new Payload(depth);

  if (depth === 0) {
    // Deepest frame: collect with ~DEPTH live roots resident on the stack.
    escape = mine;
    gc();
    escape = null;
    return mine.value();
  }

  const deeper = descend(depth - 1);
  return (mine.value() + deeper) | 0;
}

let checksum = 0;
for (let round = 0; round < ROUNDS; round++) {
  checksum = (checksum + descend(DEPTH)) | 0;
}

gc();
const mu = process.memoryUsage();

console.log("probe:11_collect_at_depth");
console.log("checksum:" + checksum);
console.error("#gcmetric heap_used_bytes=" + mu.heapUsed);
console.error("#gcmetric heap_total_bytes=" + mu.heapTotal);
console.error("#gcmetric rss_bytes=" + mu.rss);
