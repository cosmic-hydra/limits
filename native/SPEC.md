# Limits native (limits) — spec v0.1

`limits` compiles a statically typed dialect of Limits ahead of time to C11, then
to a native binary with the system C compiler (`cc`, override with `$CC`).
The frontend is a single pass per stage (lex, parse, check, emit) with no
dependencies beyond the Rust standard library.

## CLI

```
limits build file.lim [-o out] [--unchecked] [--max-steps N] [--max-depth N]
                    [--max-mem MB] [--emit-c] [--release | --debug] [--time]
limits run   file.lim [same flags]      # build to a temp dir, run, forward exit code
limits check file.lim                   # lex + parse + type check only
```

- `--release` (default) compiles with `-O2`; `--debug` with `-O0 -g`. On the
  benchmark machine `-O0` compiles about 4-6x faster than `-O2` for large
  programs and the binary runs roughly 2-5x slower (see `bench/RESULTS.md`),
  so `--debug` is the fast edit-compile-run mode.
- Large programs (more than 64 KiB of generated function code) are split into
  up to `nproc` C files that the C compiler builds in parallel and then links.
  Small programs stay one file, so the C compiler can inline across all of it.
- `--emit-c` writes the generated C to `-o` (or stdout) instead of compiling.
- `--time` prints frontend ms (with a lex+parse / check / codegen breakdown and
  the number of C files) and C compiler ms to stderr.
- Exit codes: limits itself returns 0 on success, 1 on a program error
  (lex/parse/check, or runtime error when using `run`), 2 on usage / I/O /
  C compiler failure.

## Lexical structure

As Limits v0.1: `#` comments, identifiers, ints (`1_000` allowed), floats
(`1.5`, no exponent form), strings with `\n \t \\ \"`.
Keywords: `let fn return if else while for in true false and or not break
continue struct import pub`.

## Types

| Type | C representation | Notes |
|---|---|---|
| `int` | `int64_t` | overflow is a runtime error (checked mode) |
| `float` | `double` | IEEE; float `/ 0.0` gives inf, not an error |
| `bool` | `bool` | |
| `str` | pointer to immutable UTF-8 bytes + length | `len` counts code points |
| `[T]` | pointer to growable list | reference type |
| `Name` (struct) | pointer to C struct | reference type |
| unit | `void` | functions without `-> T` |

There are no implicit conversions: use `int(x)`, `float(x)`, `str(x)`.

## Items

```
struct Point { x: float, y: float }
fn dist2(a: Point, b: Point) -> float { ... }
fn main() { ... }              # required entry point, no params, no return type
import math;                   # see Modules
pub fn f() { }  pub struct S { }   # visible to importing modules
```

Functions and structs are visible in the whole file (mutual recursion works).
A function with a return type must end in `return` on every path (an
`if`/`else` chain where every branch returns counts).

## Modules

Every `.lim` file is a module with its own namespace.

```
import math;            # loads math.lim next to the importing file
import util.strings;    # loads util/strings.lim; referred to as `strings`

let v = math.Vec2 { x: 1.0, y: 2.0 };   # struct literal from a module
let r: math.Vec2 = math.scale(v, 2.0);  # type and call from a module
print(strings.pad("x", 3));
```

- Paths are resolved relative to the directory of the importing file. The
  alias is the last path segment; importing two modules with the same last
  segment into one file is a check error.
- Only `pub fn` and `pub struct` items can be used from other modules; using a
  private one is a check error naming it. Struct fields are always accessible.
- Each file is lexed, parsed and checked exactly once per build (cached by
  canonical path), so diamond imports share one copy of the module.
- Import cycles are a check error that prints the path, e.g.
  `import cycle: a.lim -> b.lim -> a.lim`. A missing file is a check error at
  the `import`.
- Only the root file needs `fn main()`. Modules have no top-level statements or
  globals, so there is no initialisation order to worry about.
- Errors in an imported file name it: `check error in util/strings.lim at 3:5: ...`.
  Errors in the root file keep the plain `kind error at line:col` form.
- Items print with their qualified name (`math.Vec2 {x: 1.0, y: 2.0}`). The
  qualifier is the import path used by the first file that imported the module.
- C name mangling: root items are `fn__name`/`S__Name`, module items
  `fn_M4util7strings3pad` (length-prefixed segments), so equal names in
  different modules (or a root item spelled like a mangled name) never collide.
- Loading is sequential: parsing a module takes well under a millisecond, so
  spawning threads would cost more than it saves; the C compiler dominates
  build time.

## Statements

```
let x = expr;              # type inferred; variables are mutable
let xs: [int] = [];        # annotation required for an empty list
x = e;  xs[i] = e;  p.f = e;
if c { } else if d { } else { }   # conditions must be bool
while c { }
for i in a..b { }          # int range, half-open, no allocation; i is a copy
for v in list { }
break; continue; return; return e;
f(x);                      # expression statement
```

A name may be declared once per block; inner blocks may shadow.
Struct literals are not allowed directly in `if`/`while`/`for` heads (wrap in
parentheses).

## Expressions (low to high precedence)

`or`, `and`, `not`, comparisons (`== != < <= > >=`, not chainable), `+ -`,
`* / %`, unary `-`, postfix (`f(x)`, `xs[i]`, `p.f`), literals
(`[1, 2]`, `Point { x: 1.0, y: 2.0 }` — any field order, every field once).

- `+ - * / %` need both operands of the same numeric type; `+` also
  concatenates `str`. Int `/` truncates toward zero.
- `== !=` work on int, float, bool, str (by content). `< <= > >=` on int,
  float, str (bytewise).
- `and`/`or` short-circuit and take/return bool.
- Indexing is bounds checked; negative indexes are errors.

## Builtins

`print(...)` (any values, space separated, newline; lists and structs print as
`[1, 2]` / `Point {x: 1.0, y: 2.0}` with strings quoted inside), `len(list|str)`,
`push(list, v)`, `pop(list)`, `str(x)`, `int(float|str|int)`,
`float(int|str|float)`, `sqrt(float) -> float`, `assert(cond, msg?)`.
Builtin names cannot be redefined.

## Errors

Lex and parse errors stop at the first error. The checker reports **all**
type and name errors at once, sorted by position:

```
check error at 6:13: undefined variable 'y'
```

Runtime errors print `runtime error at line:col: msg` to stderr and exit 1
(stdout is flushed first): index out of bounds, division by zero, integer
overflow, pop from empty list, invalid int/float conversion, assertion failed,
step limit exceeded, call depth limit exceeded, memory limit exceeded, value
nested too deeply to format. Errors raised in an imported module read
`runtime error in m/u.lim at line:col: msg`. `limits run` exits 128+N with
`limits: program killed by signal N` if the program dies from a signal.
`int(s)`/`float(s)` accept only strict decimal syntax (optional sign; for
floats a fraction and exponent): no whitespace, hex, `inf` or `nan`. Floats
print `inf`, `-inf`, `nan`. The literal `-9223372036854775808` is allowed.
Struct names may not be `int`, `float`, `bool` or `str`, and a struct may not
contain itself except through a list.

## Limits guarantee

By default the generated code:

- checks integer overflow (`__builtin_*_overflow`), bounds and division by zero;
- charges one step on every loop iteration (back edge) and every function
  call against `--max-steps` (default 1_000_000_000). The error position is
  the loop head expression, or the called function's `fn` keyword;
- caps call depth at `--max-depth` (default 10000), reported at the called
  function's `fn` keyword. Infinite recursion therefore stops with an error
  instead of overflowing the C stack. The remaining C stack is also checked
  on every call (from `RLIMIT_STACK`, 512 KiB margin), so large frames hit the
  same error rather than a segfault;
- charges builtins doing O(n) work (string `+`, `==`, comparisons, `len(str)`,
  `str()`, `print`) one extra step per 64 bytes, and one step per value node
  formatted;
- charges every heap byte against `--max-mem` (default 2048 MB; this check
  also stays on with `--unchecked`). Formatting deeply nested or cyclic
  values stops with an error (stack-checked in all modes).

The compiler caps syntax nesting (parentheses, blocks, operator chains) at
3000 levels (`parse error ...: nesting too deep`).

`--unchecked` removes step, depth and overflow checks (integer arithmetic
wraps) for full speed. Bounds, division-by-zero and conversion checks remain.
Very deep recursion in unchecked mode can overflow the native stack.

## Known limitations (v0.1)

- Memory is bump/arena allocated and never freed (list storage uses
  `realloc`); long-running allocation-heavy programs grow without bound.
- Evaluation order of function arguments, list-literal elements and the
  operands of a string `+` chain follows C
  and is unspecified; struct literal fields are evaluated in declaration order
  only as far as C guarantees (also unspecified). Avoid side effects there.
- Runtime locations pack the file index into the line (`line | file << 20`):
  files over 1,048,575 lines report wrong positions.
- A single stack frame larger than the 512 KiB margin (tens of thousands of
  live locals) could still overflow the stack before the check fires.
- Module-qualified names are only `alias.item` in call, struct-literal and type
  position; there are no re-exports, `import x as y`, or qualified globals.
- Lists store every element as an 8-byte slot (`[bool]` included), so
  memory-bound code over large lists moves up to 8x more bytes than a packed C
  array would (see the sieve in `bench/RESULTS.md`).
- No closures or function values,
  no maps, no string indexing, no float exponent literals.
