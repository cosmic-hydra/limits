# Limits

Limits (`.lim`) is a small dynamically typed language for AI-agent computation.
Both its tree interpreter and bytecode VM enforce statement/loop/call budgets
and explicit call-depth limits without using the native stack for language calls.

```sh
cargo build --release --manifest-path compiler/Cargo.toml
compiler/target/release/lim capabilities
compiler/target/release/lim check examples/agents/task_router.lim --json
compiler/target/release/lim run examples/agents/task_router.lim --engine vm --json --sandbox restricted
```

- **Agent protocol:** versioned JSON diagnostics, partial stdout, error hints and
  a discoverable builtin/effect catalog; UTF-8 stdin supports generated source.
- **Code intelligence:** lexical semantic index, symbols, references, candidate
  definitions, captures and completion; optional standard-library-only Python LSP bridge.
- **Capability sandbox:** builtin allow/deny policies survive aliases and closures;
  aggregate output caps, cooperative deadlines and supervisor cancellation.
- **Optional encrypted source:** standard authenticated `age` encryption with
  key-based access and direct in-memory execution. There is no "AI-only" cipher.
- **Compact data operations:** `get`, `has`, `join`, `sum`; measured workload-specific
  source reduction and compiler benchmarks, without universal performance claims.

The compatibility default remains `tree`; select `--engine vm` for production
workloads. Execution budgets are not a hard RAM or wall-clock sandbox: run
untrusted workloads in supervised OS workers with memory/time and I/O limits.

Read [the agent platform contract](docs/agent-platform.md),
[agent examples and coding guide](docs/agents.md), [deployment notes](docs/production.md),
[encryption threat model](docs/encryption.md), and [benchmark methodology](bench/README.md).
[SPEC.md](SPEC.md) defines the language. `compiler/` contains the dependency-free
Rust implementation; `tools/` contains the optional Python editor integration.
The `rust/` and `cpp/` directories are upstream source snapshots, not build dependencies.
