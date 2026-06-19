// Correctness: typed-array parameter lowering must honor non-zero byte offsets
// and subarray views. This fixture is intentionally separate from the hot
// benchmark so slow setup helpers do not weaken the compiler-output gate.

const backing = new Float64Array(16);

for (let i = 0; i < backing.length; i++) {
  backing[i] = i + 0.5;
}

function sumTypedParam(input: Float64Array): number {
  let sum = 0;
  for (let i = 0; i < input.length; i++) {
    sum = sum + input[i];
  }
  return sum;
}

const offsetView = new Float64Array(backing.buffer, 3 * 8, 7);
const subarrayView = backing.subarray(4, 11);
const checksum = sumTypedParam(offsetView) + sumTypedParam(subarrayView);

if (checksum !== 98) {
  throw new Error("typedarray_param_offset checksum mismatch: " + checksum);
}

console.log("typedarray_param_offset:" + checksum);
