// Benchmark: typed-array parameter lowering
// Measures: Float64Array parameter loops that should lower to raw f64 loads.
// Catches: regressions back to per-element js_typed_array_get helper calls.

const SIZE = 131072;
const ITERATIONS = 1000;
const values = new Float64Array(SIZE);

for (let i = 0; i < SIZE; i++) {
  values[i] = (i % 97) + 0.25;
}

function sumTypedParam(input: Float64Array): number {
  let sum = 0;
  for (let i = 0; i < input.length; i++) {
    sum = sum + input[i];
  }
  return sum;
}

let warmup = 0;
for (let i = 0; i < 5; i++) {
  warmup = warmup + sumTypedParam(values);
}

const start = Date.now();
let checksum = 0;
for (let iter = 0; iter < ITERATIONS; iter++) {
  checksum = checksum + sumTypedParam(values);
}
const elapsed = Date.now() - start;

const EXPECTED = 6323324000;
if (checksum !== EXPECTED || warmup !== 31616620) {
  throw new Error("typedarray_param_sum checksum mismatch: " + checksum + "," + warmup);
}

console.log("typedarray_param_sum:" + elapsed);
console.log("checksum:" + checksum);
