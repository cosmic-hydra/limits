# Next prompts (priority 1)

Two independent tasks that can run in parallel. Each prompt is self-contained: paste it as-is. They touch different files, so they can be merged in either order; if both edit `compiler/src/ast.rs`, keep both sets of additions.

---

## Prompt A — Codex: bytecode compiler + VM

```
You are working in the `limits` repo (github.com/cosmic-hydra/limits). It contains Limits, a small dynamically typed language for AI agents with `.lim` files. Read SPEC.md first, then compiler/src/*.rs: lexer.rs -> parser.rs (AST in ast.rs) -> interp.rs (tree-walking interpreter). The `lim` CLI is compiler/src/main.rs. Static checks (undefined names, stray break/continue) live in compiler/src/resolve.rs and run before every `lim run`; any AST variant you add must be handled there too. Tests: `cargo test --manifest-path compiler/Cargo.toml` (all must stay green).

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
You are working in the `limits` repo (github.com/cosmic-hydra/limits). It contains Limits, a small dynamically typed language for AI agents with `.lim` files. Read SPEC.md first, then compiler/src/*.rs: lexer.rs -> parser.rs (AST in ast.rs) -> interp.rs (tree-walking interpreter). The `lim` CLI is compiler/src/main.rs. Static checks (undefined names, stray break/continue) live in compiler/src/resolve.rs and run before every `lim run`; any AST variant you add must be handled there too. Tests: `cargo test --manifest-path compiler/Cargo.toml` (all must stay green).

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

---

## Prompt C — Perplexity: research the hardest design questions

```
You are the research lead for Limits, a programming language used ONLY by AI agents (LLM coding agents write it, read it, run it, and repair it). Source files use the .lim extension; everything else is named "limits". Repo: github.com/cosmic-hydra/limits (read SPEC.md and the native/ compiler, if accessible).

Current state: a tree-walking interpreter and a bytecode VM (3x faster), static name checking, enforced step and call-depth budgets, plus a native compiler that compiles statically typed Limits to C11 and then to machine code via cc. Memory in the native compiler is currently a bump arena that never frees.

Goals: (1) at least 100x faster than Python on compute, (2) far fewer tokens than Python for the same task, (3) the highest first-attempt correctness for LLM-written code, (4) safe to run untrusted agent code.

Research these hard questions. For each, cite primary sources (papers, benchmarks, language docs, compiler source), label evidence strength (measured / vendor-reported / anecdotal / unknown), and end with ONE concrete recommendation plus the tradeoff you are accepting.

1. Memory management without GC pauses or programmer burden: compare reference counting with compile-time elision (Perceus/Koka, Lobster, Swift ARC), region inference (MLKit), generational references (Vale), and arenas with escape analysis. Which gives near-C speed, zero leaks for long-running agent processes, AND is easiest for an LLM to write correctly? Sketch how to implement it in a C-emitting compiler.

2. Token efficiency vs LLM accuracy: what evidence exists on how syntax choices (keywords vs symbols, significant whitespace, terse APL-style operators, type annotations, braces) affect LLM tokenizer counts AND LLM generation error rates? Measure or find data: tokens for equivalent programs in Python, Go, Rust, J/K/APL, Lua under the cl100k/o200k and Claude tokenizers. Propose the syntax rules that minimize tokens without raising error rates.

3. Designing a language for LLM first-pass correctness: what features most reduce LLM bugs (strong static types, exhaustive pattern matching, no nulls, immutability by default, contracts, effect types, unambiguous grammar, small orthogonal stdlib)? Cite studies of LLM pass@k across languages (MultiPL-E, HumanEval-X, etc.) and explain why lower-resource languages score worse and how a new language can overcome having zero training data (in-context spec size, few-shot, grammar-constrained decoding).

4. Sandboxing agent-written code: compare capability-based security (WASI, Deno permissions, object capabilities in E/Monte, Austral linear capabilities) for declaring and enforcing file, network, process and time permissions at the language level. Recommend a design where the permission set is visible in the source and enforced both statically and at runtime.

5. Ultrafast compile times: what makes compilers like TCC, Go, Zig (self-hosted backend), Jai, Cranelift and QBE fast? Should Limits keep emitting C (and use TCC for debug builds), or emit QBE IL or Cranelift IR directly? Give real compile-throughput numbers (lines/sec) from sources.

6. Structured diagnostics for agent self-repair: what error-message formats (rustc JSON, LSP diagnostics, Elm-style) lead to the highest LLM fix rates? Propose a JSON diagnostic schema for Limits.

Deliver: a single report with an executive summary (top 6 decisions in one line each), then one section per question, then a prioritized roadmap of what to build next and in which order. Never invent numbers; say "unknown" where evidence is missing.
```
