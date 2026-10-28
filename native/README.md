# limits — Limits native

Statically typed Limits compiled ahead of time to C11 and then to a native
binary. Zero crates.io dependencies. See [SPEC.md](SPEC.md).

Since v0.2 the syntax is indentation based (Python style, 4 spaces, no
semicolons) and a file's top-level statements are the program:

```
fn fib(n: int) -> int:
    if n < 2:
        return n
    fib(n - 1) + fib(n - 2)

print([fib(i) for i in 0..10], sum([fib(i) for i in 0..10]))
```

Since v0.3 there are sum types and no null: `enum` with payload variants,
exhaustive `match`, `Option[T]` / `Result[T, E]` and the `?` operator
(see `examples/shapes.lim`):

```
fn add(a: str, b: str) -> Option[int]: Some(parse_int(a)? + parse_int(b)?)

match add("1", "x"):
    Some(n): print(n)
    None: print("not numbers")
```

Since v0.4 every value is a value: lists, structs, enums and strings are
never shared (reference counted, copy-on-write, freed as soon as unused).
`let` values never change; a fn changes its caller's variable only through
an `inout` parameter, passed with `&`:

```
fn add(xs: inout [int], v: int):
    push(xs, v)

var a = [1]
let b = a          # a copy
add(&a, 2)
print(a, b)        # [1, 2] [1]
```

Since v0.5 I/O goes through capability values: the program receives
`env: Env`, and a fn that does I/O takes the capability it needs as a
parameter. The `uses` line declares what the program may do
(`limits run --show-perms` prints it):

```
uses fs.read

fn total(fs: Fs, name: str) -> int:
    sum([unwrap_or(parse_int(x), 0) for x in split(fs.read(name), "\n")])

print(total(env.fs.sub("data"), "nums.txt"))   # reads only inside ./data
```

`limits run`, `limits test` and `--debug` compile with TinyCC when
`tools/tcc/bin/tcc` (or `$LIMITS_TCC`) exists, which makes small programs
start in tens of milliseconds; `limits build` uses `cc -O2`. `--cc tcc|cc|PATH`
or `$LIMITS_CC` choose explicitly (`LIMITS_CC=cc cargo test` runs the suite
with gcc).

For agents writing Limits: [LLM.md](LLM.md) is a compact, complete
description of the language, and [grammar.ebnf](grammar.ebnf) its grammar.

`python3 tools/migrate.py old.lim...` converts v0.1 (brace) sources in place.

```
cargo build --release
target/release/limits run examples/fib.lim --time
target/release/limits build examples/primes.lim -o primes && ./primes
target/release/limits test examples/fib.lim   # inline `test "name":` blocks
target/release/limits check examples/fib.lim --json   # one JSON diagnostic per line
target/release/limits run examples/structs.lim --leak-check  # fail if memory is left at exit
cargo test
```

Layout: `src/lexer.rs` → `src/parser.rs` (AST in `src/ast.rs`) →
`src/check.rs` (names + types + mutability, all errors at once; codes and
hints in `src/diag.rs`) → `src/cgen.rs` (C emission with reference-count
insertion, with `src/runtime.h` (allocator, counts, strings, lists) embedded)
→ `cc`. The lexer emits `Newline`/`Indent`/`Dedent`
tokens; the checker turns top-level statements into the implicit `main`.
