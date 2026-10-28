# Next prompts (priority 1)

Two independent tasks that can run in parallel. Each prompt is self-contained: paste it as-is. They touch different files, so they can be merged in either order; if both edit `compiler/src/ast.rs`, keep both sets of additions.

---

## Prompt A — Codex: bytecode compiler + VM

```
You are working in the `limits` repo (github.com/cosmic-hydra/limits). It contains Limits, a small dynamically typed language for AI agents with `.lim` files. Read SPEC.md first, then compiler/src/*.rs: lexer.rs -> parser.rs (AST in ast.rs) -> interp.rs (tree-walking interpreter). The `lim` CLI is compiler/src/main.rs. Tests: `cargo test --manifest-path compiler/Cargo.toml` (11 must stay green).

The language's defining guarantee is enforced execution limits: a step budget (`Limits.max_steps`) and call-depth cap (`Limits.max_depth`). Runaway programs must stop with a `runtime` LimError carrying line:col, never hang or overflow the native stack.

TASK: add a bytecode compiler and stack VM as a second execution engine, with identical observable behaviour to the tree-walker.

Requirements:
1. New files only for the engine: compiler/src/bytecode.rs (opcodes, Chunk with constant pool and a per-instruction Span table), compiler/src/codegen.rs (AST -> Chunk, with local-slot resolution and closures via upvalues), compiler/src/vm.rs (the VM). Reuse `interp::Value` (move it to compiler/src/value.rs if cleaner, keeping a re-export so existing imports still compile).
2. The VM must enforce the same limits: charge one step per executed statement-equivalent and per call, and one per loop iteration (an empty `while true {}` must hit the step limit). Use an explicit frame stack, never Rust recursion, so depth is bounded by max_depth only.
3. Same errors, same messages, same spans as interp.rs for every case in compiler/tests/lang.rs.
4. CLI: `lim run file.lim --engine vm|tree`, default `tree` for now.
5. Differential testing: add compiler/tests/differential.rs that runs every test program and every examples/*.lim through both engines and asserts identical stdout and identical error (kind, msg, line, col). Add at least 15 new programs that stress closures capturing loop variables, nested break/continue, recursion near the depth limit, and list/map aliasing.
6. Add a benchmark example (examples/bench_fib.lim, fib(25)) and report tree vs VM wall time in the PR description. Target: VM at least 3x faster.
7. `cargo clippy --all-targets` clean, `cargo fmt` applied. No new dependencies.

Do not change language semantics or SPEC.md except to document `--engine`. If you find a tree-walker bug, fix it in interp.rs with a regression test and call it out in the PR.
```

---

## Prompt B — Kimi: error handling, modules, and a standard library

```
You are working in the `limits` repo (github.com/cosmic-hydra/limits). It contains Limits, a small dynamically typed language for AI agents with `.lim` files. Read SPEC.md first, then compiler/src/*.rs: lexer.rs -> parser.rs (AST in ast.rs) -> interp.rs (tree-walking interpreter). The `lim` CLI is compiler/src/main.rs. Tests: `cargo test --manifest-path compiler/Cargo.toml` (11 must stay green).

The language's defining guarantee is enforced execution limits (step budget and call-depth cap); every feature you add must respect them, and every error must be a LimError with an accurate line:col.

TASK: make Limits practical for writing agent logic by adding three features to the tree-walking interpreter. Another agent is building a bytecode VM in new files (bytecode.rs, codegen.rs, vm.rs); do not create those. When adding AST variants, append them at the end of each enum to keep merges easy.

1. Error handling.
   - `throw expr;` raises a user error carrying any value.
   - `try { ... } catch e { ... }` catches user errors AND runtime errors (division by zero, out of bounds, etc.). `e` is a map: {"kind": "runtime"|"user", "msg": string, "value": thrown value or nil, "line": int, "col": int}.
   - Limit errors (step limit, depth limit) must NOT be catchable: an agent must not be able to swallow its own budget. Add a test proving `try { while true {} } catch e {}` still fails with the step-limit error.
2. Modules.
   - `import "path/to/mod.lim" as name;` runs the module once (cached by canonical path) in its own global scope and binds `name` to a map of its top-level `let`/`fn` bindings. Access as `name["fn_name"](...)`; also add `name.field` member syntax that desugars to the same lookup.
   - Paths resolve relative to the importing file. Detect and reject import cycles with a clear error naming the cycle. Modules share the program's step and depth budget.
3. Standard library builtins (each with tests for normal use and wrong-type errors):
   - strings: `split(s, sep)`, `join(list, sep)`, `trim(s)`, `upper(s)`, `lower(s)`, `contains(s_or_list, x)`, `replace(s, a, b)`, `starts_with(s, p)`, `ends_with(s, p)`, `slice(s_or_list, start, end)`
   - collections: `sort(list)` (ints/floats/strings; error on mixed), `reverse(list)`, `has(map, key)`, `remove(map, key)`, `values(map)`, `map(list, fn)`, `filter(list, fn)`, `reduce(list, fn, init)`
   - math: `abs`, `min`, `max`, `floor`, `ceil`, `round`, `sqrt`
   - data: `json_parse(s)` and `json_stringify(v)` written by hand (no new dependencies), round-tripping all Limits values except fn.
   - Callbacks passed to map/filter/reduce must count against the step budget.

Also:
- Update SPEC.md with the new syntax, semantics, and builtins.
- Add examples/errors.lim, examples/modules/ (main.lim + util.lim), and examples/stdlib.lim, and assert their output in compiler/tests/lang.rs.
- `cargo clippy --all-targets` clean, `cargo fmt` applied. No new dependencies.
- In the PR description, list every design decision you made that SPEC.md did not already dictate.
```
