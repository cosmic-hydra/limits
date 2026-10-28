# Limits native (limits) — spec v0.1

`limits` compiles a statically typed dialect of Limits ahead of time to C11, then
to a native binary with the system C compiler (`cc`, override with `$CC`).
The frontend is a single pass per stage (lex, parse, check, emit) with no
dependencies beyond the Rust standard library.

## CLI

```
limits build file.lim [-o out] [--unchecked] [--max-steps N] [--max-depth N]
                    [--max-mem MB] [--emit-c] [--release | --debug] [--time] [--json]
                    [--allow fs.read=DIR | fs.write=DIR | env=NAME]...
limits run   file.lim [same flags] [-- args]  # build to a temp dir, run, forward exit code
limits check file.lim [--json]          # lex + parse + type check only
limits test  file.lim [same flags]      # run the inline tests (see Tests)
```

- `--json` prints every diagnostic (lex, parse, check and, because the flag
  is compiled into the program, runtime) to stderr as one JSON object per
  line: `{"kind","code","file","line","col","msg","hint"}`. `kind` is
  `lex|parse|check|runtime`, `file` is the path of the file the error is in
  (the root as given on the command line), `hint` is `""` when there is none.
  Output is always valid UTF-8 (invalid bytes from program strings become
  `\ufffd`); a source file that is not UTF-8 is lex error E0005. See *Diagnostic codes*.

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
continue struct import pub`. `uses`, `test`, `requires`, `ensures` and `result` are
contextual: they are ordinary identifiers except in the positions below.

## Types

| Type | C representation | Notes |
|---|---|---|
| `int` | `int64_t` | overflow is a runtime error (checked mode) |
| `float` | `double` | IEEE; float `/ 0.0` gives inf, not an error |
| `bool` | `bool` | |
| `str` | pointer to immutable UTF-8 bytes + length | `len` counts code points |
| `[T]` | pointer to growable list, elements packed by type (`[bool]` 1 byte, `[int]`/`[float]` 8, others one pointer) | reference type |
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

### Contracts

```
fn isqrt(n: int) -> int
  requires n >= 0;
  ensures result * result <= n;
{ ... }
```

Any number of `requires <bool>;` and `ensures <bool>;` clauses may follow the
signature. `requires` is checked on entry, `ensures` before every return
(including falling off the end of a unit function), with `result` bound to
the returned value (only in functions with a return type). Clauses see the
parameters (their current values at that point) and never body locals.
A failure is a runtime error at the clause: `requires failed in fn isqrt` /
`ensures failed in fn isqrt` (module functions use the qualified name).
`--unchecked` skips contracts entirely.

### Tests

```
test "isqrt of a square" { assert(isqrt(16) == 4); }
```

Top-level `test "name" { ... }` blocks are checked like unit functions in
every mode but compiled only by `limits test`, which builds the file and all
its modules (no `fn main` needed), then runs each test as its own process
(`prog <index>`), so a test that fails or exhausts a budget cannot affect
the others. Output, one line per test then a summary:

```
ok isqrt of a square
FAIL m.other: runtime error in m.lim at 3:5: assertion failed
1 passed, 1 failed
```

Tests in modules are named `module.name`. The exit code is 1 if any test
failed. Test stdout is discarded; the FAIL line carries the first stderr
line (a JSON object with `--json`). Each test gets the full step, depth and
memory budgets, and a 10 s wall-clock limit (`FAIL name: timed out after
10 s`), which matters under `--unchecked`. Names are printed escaped as in a
JSON string (without quotes: `\n`, `\"`), so a name cannot forge a result line.

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

## Capabilities and I/O

Programs are untrusted by default: without a `uses` line a program can only
compute and `print`. A file opts into I/O with a top-level declaration (any
subset, any order, may be repeated):

```
uses fs.read, fs.write, env, time, args, stdin;
```

| Builtin | Capability | Behaviour |
|---|---|---|
| `read_file(path: str) -> str` | `fs.read` | whole file |
| `write_file(path: str, data: str)` | `fs.write` | create or truncate |
| `env(name: str) -> str` | `env` | `""` if unset |
| `now_ms() -> int` | `time` | wall clock, ms since the Unix epoch |
| `args() -> [str]` | `args` | program arguments (after `--` for `limits run`) |
| `input_line() -> str` | `stdin` | one line without `\n`; `""` at end of input; all bytes kept (NUL included) |

Checks (all reported at once, like every check error):

- calling a builtin whose capability the file did not declare:
  `'read_file' needs capability 'fs.read'` (hint: `add 'uses fs.read;' ...`);
- an unknown name in `uses`;
- every module declares its own capabilities, and may only declare ones the
  root file declared: `capability 'env' is not declared by the root file
  (import chain: main.lim -> a.lim -> b.lim)`. A host can therefore read the
  root's `uses` line to know everything the program may do.

Runtime enforcement (the `--allow` grants are compiled into the binary):

- `--allow fs.read=DIR` / `--allow fs.write=DIR` (repeatable) set the
  directory trees the program may read / write; `DIR` must exist and is
  canonicalised at build time. With no grant for one of them, that access is
  limited to the process's current working directory subtree at run time.
- Every path is canonicalised before the check (`realpath`: `..` and
  symlinks are resolved, relative paths are against the working directory),
  so `../x`, `sub/../../x` and symlinks pointing outside are all denied:
  `access denied: '../x' is outside the allowed fs.read paths (see --allow)`.
  A missing final component is resolved through its directory (so a missing
  file outside the sandbox is denied, not reported as missing). The canonical
  path is then opened with `O_NOFOLLOW` and must be a regular file (no
  directories, devices or FIFOs; a FIFO is rejected without blocking).
- `--allow env=NAME` (repeatable) restricts `env` to those names; reading any
  other is `access denied`. Without it every variable is readable.
- I/O failures are runtime errors at the call: `cannot read file 'x': No such
  file or directory`, `cannot write file 'x': ...`.
- `read_file`, `write_file` and `input_line` charge one step per 64 bytes.
  Bytes read are heap and count against `--max-mem`; bytes written also count
  against `--max-mem` (never refunded), which bounds the disk a program can fill.
- Not defended against: another process swapping a directory for a symlink
  between the check and the `open` (the program itself cannot create
  symlinks); paths longer than `PATH_MAX`. Strings containing a NUL byte are
  rejected as paths.

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
`float(int|str|float)`, `sqrt(float) -> float`, `assert(cond, msg?)`, and the
I/O builtins of *Capabilities and I/O*.

Text and ordering builtins:

| Builtin | Behaviour |
|---|---|
| `split(s: str, sep: str) -> [str]` | pieces between occurrences of `sep` (like Python `s.split(sep)`: `split("", ",")` is `[""]`); an empty `sep` splits into code points (`split("hé", "")` is `["h", "é"]`) |
| `join(xs: [str], sep: str) -> str` | concatenation with `sep` between elements; `""` for an empty list |
| `sort(xs: [int] or [float] or [str])` | in place, ascending, stable; floats with NaN last; strings bytewise |

Each charges one step per 64 bytes scanned or produced plus one per piece
(`split`, `join`), or one step per element per merge pass (`sort`, about
n log2 n). `sort`'s scratch buffer (8 bytes per element) is charged to
`--max-mem`.
Builtin names cannot be redefined.

## Errors

Lex and parse errors stop at the first error. The checker reports **all**
type and name errors at once, sorted by position:

```
check error at 6:13: undefined variable 'y'
  hint: did you mean 'x'?
```

A `hint:` line follows when there is an obvious fix: the closest name in
scope by edit distance (names of 3+ chars, distance at most a third of the
length), `add 'pub' to fn f in m.lim`, `declare it first: let z = ...;`.

Runtime errors print `runtime error at line:col: msg` to stderr and exit 1
(stdout is flushed first): index out of bounds, division by zero, integer
overflow, pop from empty list, invalid int/float conversion, assertion failed,
step limit exceeded, call depth limit exceeded, memory limit exceeded, value
nested too deeply to format. Errors raised in an imported module read
`runtime error in m/u.lim at line:col: msg`. `limits run` exits 128+N with
`limits: program killed by signal N` if the program dies from a signal.
`int(s)`/`float(s)` accept only strict decimal syntax (optional sign; for
floats a fraction and exponent): no whitespace, hex, `inf` or `nan`. Floats
print `inf`, `-inf`, `nan`; every other float prints in positional
notation with the shortest round-tripping digits and never with an exponent
(`100000000000000000000.0`, `0.000001`), so printed values are valid source.
`print` formats all its arguments before writing, so an error in an argument
writes nothing for that call. The literal `-9223372036854775808` is allowed.
Struct names may not be `int`, `float`, `bool` or `str`, and a struct may not
contain itself except through a list.

## Diagnostic codes

Codes are stable. A message is assigned the first matching row (by kind and
message text) of the table in `src/diag.rs`, which is also compiled into
`--json` programs for runtime errors.

| Code | Kind | Meaning |
|---|---|---|
| E0001 | lex | integer literal out of range |
| E0002 | lex | unterminated string |
| E0003 | lex | unknown escape |
| E0004 | lex | unexpected character |
| E0005 | lex | source file is not valid UTF-8 |
| E0100 | parse | syntax error (`expected ..., found ...`) |
| E0101 | parse | nesting too deep |
| E0102 | parse | chained comparison |
| E0103 | parse | nested fn/struct definition |
| E0104 | parse | invalid assignment target |
| E0201 | check | undefined variable |
| E0202 | check | assignment to undeclared variable |
| E0203 | check | undefined function |
| E0204 | check | unknown struct or type |
| E0205 | check | unknown module alias (missing import) |
| E0206 | check | module has no such function/struct |
| E0207 | check | item is private |
| E0208 | check | function/struct defined twice |
| E0209 | check | name already declared in this block |
| E0210 | check | duplicate parameter |
| E0211 | check | duplicate struct field |
| E0212 | check | redefines a builtin function or type name |
| E0213 | check | missing `fn main()` |
| E0214 | check | bad `main` signature |
| E0215 | check | module file not found |
| E0216 | check | import cycle |
| E0217 | check | module name imported twice |
| E0218 | check | `break`/`continue` outside a loop |
| E0219 | check | builtin used without its capability |
| E0220 | check | unknown capability in `uses` |
| E0221 | check | module uses a capability the root file did not declare |
| E0300 | check | other type error |
| E0301 | check | type mismatch (`X must be T, got U`) |
| E0302 | check | operator not applicable to these types |
| E0303 | check | wrong number of arguments |
| E0304 | check | wrong argument type for a builtin |
| E0305 | check | struct has no such field |
| E0306 | check | field access on a non-struct |
| E0307 | check | indexing a non-list |
| E0308 | check | `for` over a non-list |
| E0309 | check | empty list needs a type annotation |
| E0310 | check | binding a unit value |
| E0311 | check | function may finish without returning |
| E0312 | check | `return` with a value in a unit function |
| E0313 | check | `return` without a value |
| E0314 | check | struct literal field missing or repeated |
| E0315 | check | struct contains itself |
| E0500 | runtime | other runtime error |
| E0501 | runtime | index out of bounds |
| E0502 | runtime | division by zero |
| E0503 | runtime | integer overflow |
| E0504 | runtime | pop from empty list |
| E0505 | runtime | invalid int/float conversion |
| E0506 | runtime | assertion failed |
| E0507 | runtime | step limit exceeded |
| E0508 | runtime | call depth limit exceeded |
| E0509 | runtime | memory limit exceeded / out of memory |
| E0510 | runtime | value nested too deeply to format |
| E0511 | runtime | `requires` failed |
| E0512 | runtime | `ensures` failed |
| E0513 | runtime | program killed by signal (reported by `limits run`, line 0) |
| E0514 | runtime | access denied (path outside `--allow`, env var not allowed) |
| E0515 | runtime | I/O failure (`cannot read file` / `cannot write file`) |

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
- No closures or function values, no maps, no float exponent literals.
  There is no string indexing; `split(s, "")` gives the code points as a list.
