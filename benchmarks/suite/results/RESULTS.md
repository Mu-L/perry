# suite/ Node and Bun Results (generated)

Evidence: [`public-node-bun-v1.json`](../../results/public-node-bun-v1.json) · commit `47436b9d797cd8571d6597ca3f6b697f23405682`
Perry: `perry 0.5.1277` · Node: `v22.23.1` · Bun: `1.3.14`
Policy: 5 measured samples per runtime and benchmark; incomplete or incorrect rows are rejected.

| Benchmark | Perry median | Node median | Bun median | Result |
|---|---:|---:|---:|---|
| 02_loop_overhead | 98 ms | 66 ms | 40 ms | loss vs both |
| 03_array_write | 1 ms | 7 ms | 5 ms | win vs both |
| 04_array_read | 25 ms | 11 ms | 14 ms | loss vs both |
| 05_fibonacci | 305 ms | 931 ms | 514 ms | win vs both |
| 06_math_intensive | 50 ms | 50 ms | 49 ms | mixed |
| 07_object_create | 3 ms | 5 ms | 6 ms | win vs both |
| 08_string_concat | 1 ms | 4 ms | 1 ms | mixed |
| 09_method_calls | 81 ms | 11 ms | 9 ms | loss vs both |
| 10_nested_loops | 43 ms | 18 ms | 19 ms | loss vs both |
| 11_prime_sieve | 110 ms | 5 ms | 5 ms | loss vs both |
| 12_binary_trees | 4 ms | 6 ms | 7 ms | win vs both |
| 13_factorial | 94 ms | 97 ms | 96 ms | win vs both |
| 14_closure | 47 ms | 50 ms | 50 ms | win vs both |
| 15_mandelbrot | 22 ms | 25 ms | 29 ms | win vs both |
| 16_matrix_multiply | 664 ms | 34 ms | 34 ms | loss vs both |
| bench_gc_pressure | 29 ms | 16 ms | 21 ms | loss vs both |
| bench_json_roundtrip | 218 ms | 407 ms | 227 ms | win vs both |
| bench_object_property | 130 ms | 16 ms | 16 ms | loss vs both |
| bench_int_arithmetic | 361 ms | 95 ms | 40 ms | loss vs both |
| bench_buffer_readwrite | 95 ms | 99 ms | 196 ms | win vs both |
| bench_array_grow | 21 ms | 14 ms | 9 ms | loss vs both |
| bench_string_heavy | 64 ms | 43 ms | 29 ms | loss vs both |
| bench_numeric_array_numeric | 71 ms | 5 ms | 4 ms | loss vs both |
| bench_numeric_array_downgrade | 20 ms | 7 ms | 5 ms | loss vs both |

## Summary

- Wins versus both peers: **9**
- Losses versus both peers: **13**
- Mixed or tied rows: **2**

> Historical note: the former v0.5.908 single-run commentary is archived in Git history and is not current evidence.
