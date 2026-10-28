# Limits language spec (v0.1)

Limits is a small, explicit, dynamically typed language for AI agents. Source files use the `.lim` extension. The defining feature is **enforced execution limits**: every program runs under a step budget and a call-depth cap, so a runaway agent-written program stops with an error instead of hanging or crashing the host.

## Running

```
cargo build --release --manifest-path compiler/Cargo.toml
compiler/target/release/lim run examples/fib.lim
compiler/target/release/lim run examples/runaway.lim --max-steps 1000
compiler/target/release/lim check file.lim     # lex + parse only
```

Defaults: `--max-steps 10000000`, `--max-depth 256`. Exit codes: 0 success, 1 program error, 2 usage error.

## Lexical structure

- Comments: `#` to end of line.
- Identifiers: letter or `_`, then letters, digits, `_`.
- Keywords: `let fn return if else while for in true false nil and or not break continue`.
- Literals: integers (`42`), floats (`3.14`), strings (`"a\nb"`, escapes `\n \t \\ \"`).
- Statements end with `;`. Blocks use `{ }`.

## Values

| Type | Example | Notes |
|---|---|---|
| int | `42` | 64-bit signed; overflow is a runtime error |
| float | `1.5` | 64-bit IEEE |
| string | `"hi"` | immutable, indexed by character |
| bool | `true` | |
| nil | `nil` | |
| list | `[1, "a"]` | mutable, shared by reference |
| map | `{"k": 1}` | string keys only, iterated in sorted key order |
| fn | `fn f(x) {...}` | first-class, closures capture their scope |

Only `nil` and `false` are falsy.

## Statements

```
let x = 1;            # declare (shadows in inner blocks)
x = 2;                # assign; error if x was never declared
xs[0] = 3; m["k"] = 4;
if c { } else if d { } else { }
while c { }
for x in 0..10 { }    # also lists, strings (chars), maps (keys)
break; continue;
fn name(a, b) { return a + b; }
```

## Expressions (low to high precedence)

1. `or`, then `and` (short-circuit; return an operand, not necessarily a bool)
2. `not`
3. `== != < <= > >=` (non-associative: `a < b < c` is a parse error)
4. `a..b` range (half-open list of ints)
5. `+ -` (`+` also concatenates strings and lists)
6. `* / %` (int `/` truncates; division by zero is an error)
7. unary `-`
8. calls `f(x)`, indexing `xs[i]` (negative indices count from the end; missing map key gives `nil`)

## Builtins

`print(...)`, `len(x)`, `push(list, v)`, `pop(list)`, `str(x)`, `int(x)`, `float(x)`, `type(x)`, `keys(map)`, `assert(cond, msg?)`.

## Errors

Every error reports a kind (`lex`, `parse`, `check`, `runtime`) and `line:col`. There is no exception handling yet.

Before running, `lim run` and `lim check` perform static checks and report **all** problems at once: undefined names, assignment to a name never declared with `let`, `break`/`continue` outside a loop (a loop outside a function does not count inside it), and duplicate parameters. Names declared anywhere in a block are visible throughout that block and in nested functions, so mutual recursion works; straight-line use-before-definition is caught at runtime instead.

## Not yet in v0.1

Modules/imports, error handling (`try`), structs/records, string methods, a standard library, static checking, a bytecode VM, a REPL, and an LSP. See `docs/next-prompts.md`.
