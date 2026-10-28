# Running Limits in production

Use the release binary and select the VM explicitly while the compatibility
default remains `tree`:

```sh
cargo build --release --manifest-path compiler/Cargo.toml
compiler/target/release/lim run file.lim --engine vm --max-steps 1000000 --max-depth 256
```

The CLI performs the existing static checks before execution for both engines.
Exit codes are 0 for success, 1 for a program error, and 2 for invalid usage or
host errors, including file access and JSON-response delivery failures. An
execution error includes its kind and source line/column.
Output written before an error remains visible. Output and flush failures are
reported as runtime errors for program stdout; failures delivering a JSON result
use exit 2 and may leave an incomplete response.

## Execution and lifetime guarantees

Both the tree interpreter and VM use explicit heap stacks for language calls,
loops and expression evaluation. They charge one step per executed statement,
one per attempted call after argument evaluation (including builtins), and one
per entered loop iteration. Ordinary expression operations do not each consume
a step. Both engines stop at the configured call-depth cap, which counts active
user-function calls, not builtin calls. A configured depth of 100,000 is covered
by tests on the normal Rust test-thread stack. Step counters
also handle the full u64 limit without arithmetic wraparound.

Scopes and containers are tracked by each engine. The collector marks from
active continuations/frames, operands, suspended iterators, and captured scopes.
It removes unreachable cycles, including recursive closures and self-referencing
containers. Collection begins at 1,024 tracked objects and adapts to the live
set. Destruction clears edges before releasing nodes, so deep container graphs
and error unwinding do not recurse through the native stack.

Bytecode is an opaque `CompiledProgram`, obtainable only through the fallible
compiler. Its instruction count, constant count and source spans can be read;
instructions and constants cannot be modified by an embedding caller. There is
no bytecode deserializer or external-bytecode execution API.

## Work guards beyond statement steps

Statement steps alone cannot bound the work of a single operation. Both engines
therefore apply these additional guards, reporting ordinary located errors:

| Guard | Bound | Error kind |
| --- | --- | --- |
| Source size | 16 MiB of UTF-8 bytes | `lex` |
| Recursive grammar nesting | 128 guarded grammar entries | `parse` |
| Expression AST nesting | 128 nodes | `parse` |
| Caller-built AST nesting | Depth 256 from a top-level statement at depth 0 | `parse` |
| Value rendering/comparison | 1,000,000 work items per operation | `runtime` |
| Rendered value or print call | 16 MiB of UTF-8 bytes, including print separators/newline | `runtime` |
| `join`/`sum` input | 1,000,000 elements per call | `runtime` |
| `join` result | 16 MiB of UTF-8 bytes | `runtime` |
| Materialized range | At most the configured `max_steps` elements per range | `runtime` |

The CLI reads at most the source-size cap plus one byte before rejecting an
oversized plaintext file or stdin submission with a located `lex` error and exit
1. Invalid UTF-8 and source I/O failures are host errors with exit 2; encrypted
input/provider failures also use that host-error contract. Long flat
binary/call/index chains are checked while building the AST, before a deep tree
can overflow recursive compilation or AST destruction.
Range length is checked against the configured total step cap, not the remaining
budget; materialization does not charge one step per element. The helper and
rendering guards likewise bound work separately from statement steps.

Comparison is iterative and remembers container pairs, preserving finite-value
and NaN behavior while allowing cyclic containers to be compared. Rendering is
iterative and quotes strings inside containers as before. A back-edge to a list
renders as `[...]`; a back-edge to a map renders as `{...}`. Repeated references
that are not cycles render normally. Exponentially expanded shared graphs fail
the rendering guard instead of hanging or exhausting output memory.

## Embedding

Prefer `run_with_engine` for an isolated invocation with explicit limits:

```rust
use limits::{run_with_engine, Engine, interp::Limits};
let mut output = Vec::new();
run_with_engine(
    "print(42);",
    &mut output,
    Limits { max_steps: 1000, max_depth: 64 },
    Engine::Vm,
).unwrap();
assert_eq!(output, b"42\n");
```

As with the original `run` library API, undefined names are reported at runtime;
call `resolve::check` separately when CLI-style static diagnostics are wanted.
The library does not flush the caller's writer.

A compiled program can be cloned cheaply and executed repeatedly. Every
`Vm::run` starts with fresh globals, a fresh step budget, and a fresh heap,
including after a previous run failed. `Interpreter` retains its original
stateful globals and cumulative step/output budgets across direct `run` calls,
including after runtime errors; its cooperative deadline restarts on each run.
Construct a fresh interpreter, or use `run_with_engine`, for isolation. These
types contain `Rc` and are not `Send`/`Sync`: create separate engines inside worker
threads or processes instead of sharing an engine concurrently.

## Host resource controls

Execution steps and graph collection are **not a hard RAM limit or a wall-clock
sandbox**. A live list can grow rapidly within a small number of statements;
strings, ranges, parser tokens and explicit call frames also consume memory.
A caller-provided `Write` implementation can block. The engine has no network or
filesystem builtins, but an untrusted workload still needs a supervised worker
process with an OS/container memory limit, CPU/time deadline, bounded I/O and a
kill/restart policy. Never translate a tenant's requested limit directly into an
unbounded host allocation budget.

Do not rely on catching Rust panics to recover from host out-of-memory failures.
The host should cap source submissions, keep configured execution limits within
its worker capacity, and distinguish program errors from worker termination.

## Verification and performance

CI configures debug/release tests, clippy with warnings denied, formatting checks
and Python protocol/LSP tests on Linux, Windows and macOS. A separate Linux job
installs `age` and runs the real authenticated-encryption integration tests. The
repository includes differential tests of language programs and every `.lim`
example, seeded generated programs, deep recursion and
container cleanup, GC root preservation, I/O failures, input guards, and a
subprocess watchdog for runaway loops.

For benchmark comparisons, build once in release mode, warm both engines, then
alternate engine order for repeated runs of `examples/bench_fib.lim`. Compare
median wall times including the same startup and output handling in both runs.
The PR records measured timings and environment; performance is workload and
host dependent, not an acceptance-time assertion in CI.
