# limits — Limits native

Statically typed Limits compiled ahead of time to C11 and then to a native
binary. Zero crates.io dependencies. See [SPEC.md](SPEC.md).

```
cargo build --release
target/release/limits run examples/fib.lim --time
target/release/limits build examples/primes.lim -o primes && ./primes
target/release/limits test examples/fib.lim   # inline `test "name" { }` blocks
target/release/limits check examples/fib.lim --json   # one JSON diagnostic per line
cargo test
```

Layout: `src/lexer.rs` → `src/parser.rs` (AST in `src/ast.rs`) →
`src/check.rs` (names + types, all errors at once; codes and hints in
`src/diag.rs`) → `src/cgen.rs` (C emission,
with `src/runtime.h` embedded) → `cc`.
