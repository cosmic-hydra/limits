# Phase benchmarks

Build and run the dependency-free phase driver from the repository root:

```sh
cargo run --release --manifest-path compiler/Cargo.toml --example bench_pipeline -- --samples 5 --width 8000
```

The driver emits one JSON object per workload. It measures lexing, parsing,
static checking, bytecode generation, and tree/VM execution separately. Each
workload gets one warmup and the requested number of measured samples; reported
values are medians in nanoseconds. Execution uses fresh engines and output
buffers, alternates engine order, and verifies exact output parity. Parsing
consumes an already lexed token vector; bytecode generation consumes an already
parsed AST. These are phase timings, not end-to-end CLI wall times.

`wide_scope` generates 8,000 declarations followed by 8,000 assignments and a
print. `fib25` uses `examples/bench_fib.lim`. Source bytes and physical nonblank,
noncomment LOC are reported as size facts. Token counts refer to Limits lexer
tokens, not an LLM tokenizer. Instruction/constant counts describe the top-level
chunk, excluding nested function chunks. LOC is not a measure of correctness or
an assertion of superior expressiveness over another language.

## Measured compile-time improvement

On this x86_64 Linux workspace with rustc 1.99.1, five-sample release measurements
of the generated wide-scope program (243,577 source bytes, 16,001 nonblank,
noncomment LOC, 88,008 lexer tokens) gave:

| Bytecode generation | Median |
| --- | ---: |
| Before indexed scope layouts | 392.662 ms |
| After indexed/shared scope layouts | 5.824 ms |
| Workload-specific speedup | 67.42× |

Raw medians for every measured phase and both workloads are retained in
`codegen-before.jsonl` and `codegen-after.jsonl`. An earlier optimized five-sample
run is preserved separately as `codegen-after-initial.jsonl` (4.274 ms); this
variation is why timing is evidence, not a performance guarantee. The optimization replaces linear
name searches per declaration/reference with hash lookup, preserves deterministic
source-order slot numbers, and shares immutable parent layouts through `Rc`
instead of cloning all parent names for nested functions. Parameter membership
checks are constant time; wide duplicate-parameter detection is linear on average.

This addresses quadratic behavior in wide lexical scopes. It does not establish
that every program compiles faster: on the tiny fib source this short run measured
12.5 µs before and 10.3 µs in the final run (24.4 µs in the initial optimized run) for bytecode generation. Such microsecond timings
are noisy, and hash table setup can cost more than scanning a tiny scope. The
result is a scale-specific improvement, not a universal language comparison.
Other agents were developing runtime features during measurement, so execution
phase changes in these snapshots should not be attributed to this compiler-only
optimization.

Use the same hardware, toolchain, input width, and sample count for comparisons.
Do not turn wall-time numbers into CI pass/fail thresholds. Correctness tests in
`compiler/tests/performance_regressions.rs` cover the wide shapes, parameter
ordering, duplicate parameter behavior, late captures, and redeclarations without
flaky timing assertions.

## Intra-Limits LOC and token examples

The benchmark also checks paired agent-report workflows with identical input
and output. The manual version uses loops for map fallback, membership, string
joining and numeric accumulation. The helper version uses `get`, `has`, `join`
and `sum`, retaining one statement per line instead of minifying source.

| Paired workflow | Noncomment LOC | Limits lexer tokens | File bytes |
| --- | ---: | ---: | ---: |
| `agent_compact/report_manual.lim` | 28 | 143 | 628 |
| `agent_compact/report_helpers.lim` | 8 | 83 | 325 |
| Existing `examples/agent_task.lim` | 17 | 115 | 525 |
| `agent_compact/actions_compact.lim` | 14 | 116 | 481 |

The report fixture reduces physical LOC by **71.4%** and lexer tokens by **42.0%**.
Parity tests include missing keys, present nil values, and false-valued fields,
so shorter syntax does not silently substitute truthiness for membership.
Both engines produce `untitled true search -> read -> answer 9` for the base data.

The action-selection rewrite consolidates the affordability and score guards.
It is an intentional counterexample: fewer physical lines do not guarantee fewer
tokens. Neither fixture supports an assertion of universal LOC superiority over
Python or any other language. File bytes include comments; lexer token counts
exclude comments and do not estimate any particular LLM's tokenization.
