# limits
programming language for AI agents

Limits (`.lim`) is a small, explicit language whose programs always run under enforced step and call-depth budgets, so agent-written code fails loudly instead of hanging.

```
cargo build --release --manifest-path compiler/Cargo.toml
compiler/target/release/lim run examples/hello.lim
```

- `SPEC.md` — the language definition
- `compiler/` — reference interpreter (Rust): lexer, parser, tree-walking interpreter, `lim` CLI
- `native/` — Limits native: statically typed Limits compiled to C, then to machine code (`limits build|run|check file.lim`); see `native/SPEC.md` and `native/bench/RESULTS.md`
- `examples/` — sample `.lim` programs
- `docs/next-prompts.md` — the next work items, written as prompts for coding agents
- `rust/`, `cpp/` — upstream Rust, LLVM and GCC source snapshots for reference
