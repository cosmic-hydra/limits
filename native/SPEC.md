# Limits native (limits) — spec v0.5

`limits` compiles a statically typed dialect of Limits ahead of time to C11, then
to a native binary with the system C compiler (`cc`, override with `$CC`).
The frontend is a single pass per stage (lex, parse, check, emit) with no
dependencies beyond the Rust standard library.

v0.2 replaced braces and semicolons with indentation, added script mode,
`let`/`var`, compound assignment, implicit returns, ranges as values, list
comprehensions and `sum`/`min`/`max`/`any`/`all`. `tools/migrate.py`
converts v0.1 sources (see *Migrating from v0.1*).

v0.3 added sum types: `enum` declarations with payload variants, `match`
with exhaustiveness checking, the built-in generics `Option[T]` and
`Result[T, E]`, the postfix `?` operator, and soft-failing builtins
(`parse_int`, `parse_float`, `find`, `get`). There is no null: a value that
may be absent is an `Option`. v0.2 programs compile unchanged unless they use
`enum` or `match` as a name, or define a function named like a new builtin.

v0.4 made every value a value: lists, structs, enums and strings are never
shared, so assigning or passing one behaves like a copy (implemented with
reference counts and copy-on-write, so copies are free until written). A `let`
value cannot change at all (not even its elements); a `var` can. Parameters
are read-only; a fn changes its caller's variable only through an `inout`
parameter, and the call marks that argument with `&` (`push(xs, v)` and the
other mutating builtins need no `&`). Memory is freed as soon as the last
reference goes away; `--max-mem` limits live bytes and `--leak-check` reports
anything left at exit. See *Values, mutation and memory*. v0.3 programs that
change a `let` value or a parameter get a check error with the fix as a hint.

v0.5 made capabilities values. There are no ambient I/O builtins: the
program's entry receives `env: Env` (in script mode a predeclared `env` at
the top level of the root file; `fn main(env: Env)` otherwise), and I/O is a
method call on a capability taken from it: `env.fs.read(path)`,
`env.stdin.read_line()`. A fn that does I/O takes the capability it needs as
a parameter (`fn load(fs: Fs)`); `fs.sub("data")` narrows an `Fs` to a
directory. The `uses` line still declares what the program may do. See
*Capabilities and I/O*. `limits run`, `limits test` and `--debug` compile
with TinyCC when it is available (see *CLI*).

`grammar.ebnf` is the complete grammar (checked against every `.lim` file
in the repository by `tools/grammar_check.py`, run from `cargo test`);
`LLM.md` is a compact description of the language for agents.

```
struct Point:
    x: float
    y: float

fn norm2(p: Point) -> float: p.x * p.x + p.y * p.y

fn isqrt(n: int) -> int:
    requires n >= 0
    var r = 0
    while (r + 1) * (r + 1) <= n:
        r += 1
    r                                   # last expression = return value

let pts = [Point { x: float(i), y: 1.0 } for i in 0..4]
print(sum([norm2(p) for p in pts if p.x > 0.0]), isqrt(17))
```

## CLI

```
limits build file.lim [-o out] [--unchecked] [--max-steps N] [--max-depth N]
                    [--max-mem MB] [--emit-c] [--release | --debug] [--cc tcc|cc|PATH]
                    [--time] [--json] [--leak-check] [--show-perms]
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

- C compiler. `limits build` compiles with `cc -O2` (`$CC` overrides `cc`).
  `limits run`, `limits test` and `--debug` use TinyCC when it is available:
  `$LIMITS_TCC`, else `tools/tcc/bin/tcc` in a directory above the limits
  executable (the repository layout); otherwise they use `cc` (`-O2`, or
  `-O0 -g` with `--debug`). `--release` forces `cc -O2`. `--cc tcc`, `--cc
  cc` or `--cc PATH` (a gcc-compatible compiler) choose explicitly, and
  `$LIMITS_CC` sets the same choice for every command. For tcc the generated
  C includes a shim for `__builtin_{add,sub,mul}_overflow` (int64 only) and
  is linked with `-lm`. tcc compiles 10-50x faster than `cc` but does not
  optimise (compute-heavy programs run up to 10x slower than `-O2`; see
  `bench/RESULTS.md`). If tcc fails (it has fixed internal limits, e.g. an
  expression of thousands of chained operators), limits silently rebuilds
  with `cc -O0`; `LIMITS_TCC_STRICT=1` disables that fallback and shows
  tcc's messages.
- Large programs (more than 64 KiB of generated function code) are split into
  up to `nproc` C files that the C compiler builds in parallel and then links.
  Small programs stay one file, so the C compiler can inline across all of it.
- `--emit-c` writes the generated C to `-o` (or stdout) instead of compiling.
- `--leak-check` compiles a check into the program: when it finishes
  normally with heap blocks still live, it prints `leak check: N heap blocks
  still live at exit` to stderr and exits 1. Every example, test
  and benchmark in this repository runs clean under it (`cargo test` checks).
- `--show-perms` prints the program's capability manifest to stdout and
  exits 0 without compiling or running: the root file's `uses` line (which
  bounds every module's), the `--allow` grants that apply to it, and each
  module's own `uses`:

  ```
  program: tool.lim
  uses: fs.read, stdin
    fs.read: current directory at run time
  module util.lim: uses fs.read
  ```
- `--time` prints frontend ms (with a lex+parse / check / codegen breakdown and
  the number of C files) and C compiler ms to stderr.
- Exit codes: limits itself returns 0 on success, 1 on a program error
  (lex/parse/check, or runtime error when using `run`), 2 on usage / I/O /
  C compiler failure.

## Lexical structure

`#` comments, identifiers, ints (`1_000` allowed), floats (`1.5`, no
exponent form), strings with `\n \t \\ \"`.
Keywords: `let var fn return if elif else while for in true false and or not
break continue pass struct enum match import pub`. `uses`, `test`, `requires`, `ensures`
and `result` are contextual: they are ordinary identifiers except in the
positions below.

Layout (Python style):

- One statement per line; there are no semicolons. A line ending in `:` opens
  a block: the following lines are indented exactly 4 spaces more, and the
  block ends at the first line indented less. After `:` a single simple
  statement may instead follow on the same line (`if x < 0: return 0`).
- Inside `()`, `[]` and `{}` line breaks and indentation are ignored, so long
  expressions, argument lists and literals may span lines.
- Blank lines and comment-only lines are ignored at any indentation.
- A tab character outside a string is lex error E0006. Indentation that is
  not one 4-space step deeper, or that does not return to an enclosing level,
  is lex error E0007. An indented line that does not follow a `:` line is a
  parse error (`expected ..., found 'indent'`).
- `pass` is the empty statement (`while busy(): pass`).

## Types

| Type | C representation | Notes |
|---|---|---|
| `int` | `int64_t` | overflow is a runtime error (checked mode) |
| `float` | `double` | IEEE; float `/ 0.0` gives inf, not an error |
| `bool` | `bool` | |
| `str` | pointer to a counted header + UTF-8 bytes | value; `len` counts code points |
| `[T]` | pointer to a counted growable list, elements packed by type (`[bool]` 1 byte, `[int]`/`[float]` 8, others one pointer) | value (copy-on-write) |
| `Name` (struct) | pointer to a counted C struct | value (copy-on-write) |
| `Name` (enum) | pointer to a counted tagged C struct (unit variants are static constants) | immutable value; see *Sum types* |
| `Option[T]`, `Result[T, E]` | like an enum, one C type per instantiation (monomorphized) | the only generic types |
| `Env`, `Fs`, `Vars`, `Clock`, `Args`, `Stdin` | `int64_t` handle | capabilities; see *Capabilities and I/O* |
| unit | `void` | functions without `-> T` |

There are no implicit conversions: use `int(x)`, `float(x)`, `str(x)`.

## Items and script mode

```
struct Point:                     # fields one per line, all typed
    x: float
    y: float
fn dist2(a: Point, b: Point) -> float:
    ...
fn sq(x: int) -> int: x * x       # one-line body
fn scale(p: inout Point, k: float):  # may change the caller's variable: scale(&q, 2.0)
    ...
enum Shape:                       # see Sum types
    Circle(r: float)
    Empty
import math                       # see Modules
pub fn f(): pass                  # `pub fn` / `pub struct` / `pub enum`: visible to importers
```

**Script mode.** Statements outside any `fn`/`struct`/`test` are the program
body: they run top to bottom when the program starts, as the body of an
implicit `fn main()`. Functions and structs may be defined anywhere in the
file (before or after their use). Top-level variables are locals of that
body, not globals: functions cannot see them. `fn main()` (no parameters, no
return type) is still accepted as the entry point when the file has no
top-level statements; `fn main(env: Env)` is the same with the program's
capabilities (the top level of a script sees them as the predeclared,
immutable `env`). Having both is check error E0224, and a file with
neither is E0213. Imported modules may not contain top-level statements
(E0223).

Type annotations are required on fn parameters, fn return types and struct
fields; local variables are inferred.

**Implicit return.** In a fn with a return type, if the body's last statement
is an expression statement, it is the returned value (`fn sq(x: int) -> int:
x * x`). This also applies to the last statement of every branch of a final
`if`/`elif`/`else` that has an `else`. `return e` still works anywhere.

### Contracts

```
fn isqrt(n: int) -> int:
    requires n >= 0
    ensures result * result <= n
    ...
```

Any number of `requires <bool>` and `ensures <bool>` lines may open the fn
body (before any other statement; there `requires`/`ensures` followed by
anything but an assignment, `.` or `[` is a clause). `requires` is checked on entry, `ensures` before every return
(including falling off the end of a unit function), with `result` bound to
the returned value (only in functions with a return type). Clauses see the
parameters (their current values at that point) and never body locals.
A failure is a runtime error at the clause: `requires failed in fn isqrt` /
`ensures failed in fn isqrt` (module functions use the qualified name).
`--unchecked` skips contracts entirely.

### Tests

```
test "isqrt of a square": assert(isqrt(16) == 4)
test "longer":
    let r = isqrt(99)
    assert(r == 9)
```

Top-level `test "name":` blocks are checked like unit functions in
every mode but compiled only by `limits test`, which builds the file and all
its modules (top-level statements are checked but not run), then runs each test as its own process
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
A function with a return type must return on every path (`return e`, or an
implicit return; an `if`/`elif`/`else` chain where every branch returns
counts).

## Sum types

```
enum Shape:
    Circle(r: float)              # payload fields: name: type
    Rect(w: float, h: float)
    Point(float, float)           # names may be omitted
    Empty                         # unit variant

let s = Circle(1.0)               # construct: positional, like a call
let t = Shape.Empty               # `Enum.Variant` also works
fn area(s: Shape) -> float:
    match s:
        Circle(r): 3.14 * r * r   # binds the payload fields in order
        Rect(w, _): w * w         # `_` ignores a field
        Point: 0.0                # no parentheses: payload ignored
        _: 0.0                    # wildcard
```

- An enum has one or more variants, one per line; field types may be any
  type, including the enum itself (recursive types such as expression
  trees and linked lists work). Enum values are immutable: build a new one.
- Variant names are module items: within a module they must be distinct
  from each other and from functions, structs and enums (E0208), and the
  bare name refers to the variant. From another module write `m.Circle(..)`
  / `m.Empty` (the enum must be `pub enum`) and the type `m.Shape`.
- `print`/`str` show `Circle(1.0)`, `Empty`, `Some("x")`, `None`,
  `Ok(1)`, `Err("bad")` (module variants qualified like structs:
  `geo.Circle(1.0)`). Printed sum values are valid source.
- Sum types have no `==`, `<` or other operators (E0302, with a hint): use
  `match` or `is_some`/`is_none`/`is_ok`/`is_err`.

**Option and Result.** `Option[T]` has variants `Some(value: T)` and
`None`; `Result[T, E]` has `Ok(value: T)` and `Err(error: E)`. They are the
only generic types (no user generics) and are monomorphized: `Option[int]`
and `Option[str]` are separate C types. Like `[]`, a constructor whose type
arguments cannot be seen needs an expected type: an annotation, a parameter,
field or return type, or the other branch of a match. `let x = None` and
`let r = Ok(1)` are check error E0316 (`cannot infer the type of 'None'`),
while `var best: Option[int] = None`, `return Ok(1)` in a fn returning
`Result[int, str]`, and `Some(3)` (type from its payload) are fine.

**match.** `match e:` followed by indented arms `pattern: body` (a body is a
one-line statement or an indented block). Arms are tried top to bottom.

| Pattern | Matches |
|---|---|
| `Circle(r)`, `Some(x)`, `Err(e)` | that variant, binding each field (names are new `let` bindings; `_` skips one) |
| `Empty`, `None`, `Circle` | that variant (a payload, if any, is ignored) |
| `3`, `-1`, `"add"`, `true` | an int, str or bool equal to the literal |
| `_` | anything |

Patterns are not nested (`Some(Circle(r))` is not allowed: match twice).
Checks, all reported at once:

- **Exhaustiveness** (E0317): a match on an enum, Option or Result must cover
  every variant or have a `_` arm; the error lists the missing variants and
  the hint spells out arms to add (`add 'Rect(w, h): ...', 'Empty: ...' (or a
  final '_: ...' arm)`). A match on int or str must end with a `_` arm; a
  match on bool needs `true` and `false` or `_`.
- **Unreachable arms** (E0318): an arm after `_`, or a variant/literal that
  is already matched.
- **Bad patterns** (E0319): an unknown variant (with a did-you-mean hint or
  the list of variants), a wrong number of bindings, a literal of the wrong
  type, matching a type that is not an enum, Option, Result, int, str or bool.

A function whose last statement is a match returns from every arm
(implicit return applies to each arm's last expression), and a match where
every arm returns counts as returning.

**match as a value.** As the whole right side of `let`, `var`, `=` or
`return`, a match produces a value:

```
let v = match parse_int(s):
    Some(n): n
    None: return -1              # an arm may leave by return / break / continue
let msg = match code:
    0: "ok"
    _:
        let m = "error " + str(code)
        m                        # a block arm ends with its value
```

Every arm must end with an expression of the same type (E0301) or leave by
`return`, `break` or `continue` (E0321 otherwise). Elsewhere (e.g. inside a
call) `match` is a parse error with a hint.

**The `?` operator.** `e?` on an `Option[T]` gives the `T` inside `Some`, or
returns `None` from the enclosing function; on a `Result[T, E]` it gives the
`Ok` value or returns `Err(error)`. The function must return `Option[_]`
(for an Option) or `Result[_, E]` with the same `E` (for a Result), else
E0320. An early return through `?` checks `ensures` like any return.

```
fn add(a: str, b: str) -> Option[int]: Some(parse_int(a)? + parse_int(b)?)
```

## Values, mutation and memory

```
var a = [1, 2]
let b = a                  # b is a copy: nothing is shared
push(a, 3)                 # a == [1, 2, 3], b == [1, 2]
var grid = [[0, 0], [0, 0]]
grid[1][0] = 7             # changes grid only
fn bump(c: inout Counter, k: int):
    c.n += k               # changes the caller's variable
bump(&counters[2], 5)      # `&` marks the argument the call may change
```

- **Values.** Lists, structs, enums and strings are values, like ints:
  assignment, passing an argument, storing into a list or field, and
  returning all behave like a copy. No two names ever share one value, so
  there are no aliases and no cycles (`push(xs, xs)` stores a copy of the old
  `xs`). Copies are cheap: they share storage through a reference count, and
  a value is copied only when it is written while another name still uses it
  (copy-on-write). Writing a value nobody else uses changes it in place.
- **What may change.** A `var` (and an `inout` parameter) may be reassigned
  and changed in place: `xs[i] = v`, `p.f = v`, `xs[i].f += v`, `push(xs, v)`,
  `pop(xs)`, `sort(xs)`. A `let` value, a parameter and a loop variable cannot
  change at all (E0222, with a hint: `declare it with 'var xs = ...'`; for a
  parameter `declare 'xs: inout [int]' and call with '&xs'`, or copy it
  with `var xs = xs`; a loop variable is a copy of the element, so change
  `xs[i]` instead). The target of a change must be a variable, or a field or
  element of one (`push(f(), 1)` is E0222).
- **Parameters** are read-only and cost nothing to pass (the callee borrows
  the caller's value). `name: inout T` hands the caller's variable to the
  fn for the duration of the call (copy-in/copy-out; no reference survives
  the call): assignments and in-place changes to it are seen by the caller
  when the call returns. The argument must be written `&place`
  where place is a `var`, an `inout` parameter, or a field or element of one
  (`f(&xs)`, `f(&p.items)`, `f(&grid[i])`). Errors (E0225, with hints): an
  inout parameter given a plain argument (`write '&xs'`), `&` on a parameter
  that is not inout, `&` anywhere except a call argument, and the same
  variable passed to two inout parameters of one call. `push`, `pop` and
  `sort` change their list argument without `&` (`push(&xs, v)` is also
  accepted).
- **Evaluation order.** Operands that do not change anything are evaluated in
  an unspecified order (as in C). When an operand changes a variable through
  `&` or `push`/`pop`/`sort`, all operands of that call or operator are
  evaluated left to right, and an operand that reads the changed variable
  keeps the value it read: `f(xs, clear(&xs))` passes the old `xs`. Places are
  resolved after every other operand (`xs[i] = g(&xs)` evaluates `i`, then
  `g`, then writes).
- **Memory.** Heap values are reference counted (not atomically: programs
  are single-threaded) and freed when their count reaches zero. The compiler
  inserts the count updates: a local is released right after the statement
  holding its last use, and that use moves the value instead of copying the
  reference when it is the only use in a simple statement (`let`, assignment,
  expression statement, `return`). Early exits (`return`, `break`,
  `continue`, `?`) release everything they leave behind. Each type gets a
  generated free function; freeing a long chain (a 10^6-element linked list,
  a deep tree) runs as a loop, never as C recursion. String and list literals
  and unit variants are static and never freed. `s += x` and `s = s + x + ...`
  append in place when `s` is not shared, so building a string in a loop is
  linear. Three optimizations remove most count and copy checks without
  changing meaning: a `let` copy of a part of a variable that the rest of
  the block never changes (`let b = bs[i]` in a read-only loop) borrows it;
  consecutive scalar writes through one path (`bs[i].x += ...;
  bs[i].y += ...`) check that `bs` and `bs[i]` are unshared only once; and a
  variable that a loop writes through but uses only through its parts
  (`xs[i]`, `p.f`, `len(xs)`, `push(xs, ..)`) is made unshared once before
  the loop instead of at every write.
- **Allocator.** Blocks up to 1 KiB come from free lists in 16-byte size
  classes (refilled from 1 MiB chunks, which are kept for reuse); larger
  blocks use `malloc`/`free`.

## Modules

Every `.lim` file is a module with its own namespace.

```
import math             # loads math.lim next to the importing file
import util.strings     # loads util/strings.lim; referred to as `strings`

let v = math.Vec2 { x: 1.0, y: 2.0 }    # struct literal from a module
let r: math.Vec2 = math.scale(v, 2.0)   # type and call from a module
print(strings.pad("x", 3))
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
- Only the root file has a program body (top-level statements or `fn main()`).
  Modules have no top-level statements (E0223) or globals, so there is no
  initialisation order to worry about.
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

Programs are untrusted by default: there are no ambient I/O functions. The
program's entry receives one value `env: Env` holding every capability; a
function can do I/O only through a capability value it was passed. `print`
(stdout) is always allowed.

```
uses fs.read, stdin

fn count_lines(fs: Fs, path: str) -> int: len(split(fs.read(path), "\n"))

let data = env.fs.sub("data")         # an Fs narrowed to ./data
print(count_lines(data, "a.txt"))     # reads data/a.txt
print(env.stdin.read_line())
```

- **Where `env` exists.** In script mode the root file's top-level
  statements see a predeclared immutable `env: Env`; functions do not (it is
  a local of the implicit `main`; using it in a fn is E0201 with a hint). With
  `fn main(env: Env)` it is that parameter. Modules and tests get no `Env`:
  they receive capabilities from their callers.
- **Capability types and methods.** `Env` has one field per capability; the
  others have methods. A method call `x.m(args)` exists only on capabilities.

| Expression | Type | Needs `uses` | Behaviour |
|---|---|---|---|
| `env.fs` | `Fs` | | the file system (limited by `--allow`) |
| `env.vars` / `env.clock` / `env.args` / `env.stdin` | `Vars` / `Clock` / `Args` / `Stdin` | | |
| `fs.read(path: str) -> str` | | `fs.read` | whole file |
| `fs.write(path: str, data: str)` | | `fs.write` | create or truncate |
| `fs.sub(dir: str) -> Fs` | | `fs.read` or `fs.write` | an `Fs` narrowed to the existing directory `dir` (inside `fs`) |
| `vars.get(name: str) -> str` | | `env` | environment variable; `""` if unset |
| `clock.now_ms() -> int` | | `time` | wall clock, ms since the Unix epoch |
| `args.list() -> [str]` | | `args` | program arguments (after `--` for `limits run`) |
| `stdin.read_line() -> str` | | `stdin` | one line without `\n`; `""` at end of input; all bytes kept (NUL included) |

- **Capabilities are not data.** They can be bound with `let`/`var`, passed
  as parameters and returned, but not stored in a struct field, enum variant,
  list or Option/Result (E0226, also for type annotations such as `[Fs]`),
  printed, converted with `str`, or compared.
- **Narrowing, never widening.** `fs.sub(dir)` resolves `dir` like a path
  of `fs` (relative paths from its root), requires an existing directory
  (`cannot use directory 'x': ...`) and returns an `Fs` whose every access,
  including further `sub`, must stay inside that directory after
  canonicalisation (`..`, absolute paths and symlinks pointing out are
  denied: `access denied: '../x' is outside the Fs root '/abs/data'`). Paths
  given to a narrowed `Fs` are relative to its root. The `--allow` limits
  below apply to every `Fs` as well. There is no way to get a wider `Fs` from
  a narrower one.

**Declaring.** A file still opts into I/O with a top-level `uses` line (any
subset, any order, may be repeated):

```
uses fs.read, fs.write, env, time, args, stdin
```

Checks (all reported at once, like every check error):

- calling a capability method whose capability the file did not declare:
  `'Fs.read' needs capability 'fs.read'` (E0219, hint: `add 'uses fs.read'
  ...`); a method that does not exist (E0227, with a did-you-mean hint);
- an unknown name in `uses` (E0220);
- every module declares its own capabilities, and may only declare ones the
  root file declared: `capability 'env' is not declared by the root file
  (import chain: main.lim -> a.lim -> b.lim)` (E0221). A host can therefore
  read the root's `uses` line (`limits run --show-perms`) to know everything
  the program may do;
- the v0.4 I/O builtins (`read_file`, `write_file`, `env`, `now_ms`, `args`,
  `input_line`) are undefined functions whose hint names the replacement.

Runtime enforcement (the `--allow` grants are compiled into the binary):

- `--allow fs.read=DIR` / `--allow fs.write=DIR` (repeatable) set the
  directory trees the program may read / write; `DIR` must exist and is
  canonicalised at build time. With no grant for one of them, that access is
  limited to the process's current working directory subtree at run time.
- Every path is canonicalised before the check (`realpath`: `..` and
  symlinks are resolved, relative paths are against the working directory
  or the narrowed `Fs` root), so `../x`, `sub/../../x` and symlinks pointing
  outside are all denied:
  `access denied: '../x' is outside the allowed fs.read paths (see --allow)`.
  A missing final component is resolved through its directory (so a missing
  file outside the sandbox is denied, not reported as missing). The canonical
  path is then opened with `O_NOFOLLOW` and must be a regular file (no
  directories, devices or FIFOs; a FIFO is rejected without blocking).
- `--allow env=NAME` (repeatable) restricts `vars.get` to those names;
  reading any other is `access denied`. Without it every variable is readable.
- I/O failures are runtime errors at the call: `cannot read file 'x': No such
  file or directory`, `cannot write file 'x': ...`.
- `fs.read`, `fs.write` and `stdin.read_line` charge one step per 64 bytes.
  Bytes read are heap and count against `--max-mem`; bytes written also count
  against `--max-mem` (never refunded), which bounds the disk a program can fill.
- Not defended against: another process swapping a directory for a symlink
  between the check and the `open` (the program itself cannot create
  symlinks); paths longer than `PATH_MAX`. Strings containing a NUL byte are
  rejected as paths. Each distinct `sub` directory is kept until exit (a few
  hundred bytes).

## Statements

```
let x = expr               # immutable binding, type inferred
var n = 0                  # mutable binding
var xs: [int] = []         # an empty list literal needs an annotation
n = e                      # only a `var` can be reassigned
n += e                     # also -= *= /= %=; `+=` concatenates str
xs[i] = e   p.f = e        # element / field assignment (any binding)
xs[i] += e  p.f *= e       # list and index (or object) evaluated once
if c:                      # conditions must be bool
    ...
elif d:                    # `else if d:` is also accepted
    ...
else:
    ...
while c:
    ...
for i in a..b:             # int range, half-open, no allocation
    ...
for v in list:
    ...
match e:                   # see Sum types
    Some(x): ...
    None: ...
break   continue   pass   return   return e
f(x)                       # expression statement
```

- `let` binds immutably and `var` mutably. Assigning to or changing a `let`
  value, a parameter or a loop variable is check error E0222, with a hint
  (`declare it with 'var x = ...'`; for a parameter `var n = n` at the start of
  the body shadows it with a mutable copy, or `n: inout T` changes the
  caller's variable). Values are never shared, so `let xs = [1]` cannot
  change at all: neither `xs[0] = 2` nor `push(xs, 2)` is allowed (see
  *Values, mutation and memory*).
- Compound assignment `t op= e` type checks like `t = t op e` and reports
  arithmetic errors (overflow, division by zero) at the operator. For
  `xs[i] op= e` and `p.f op= e` the list/object and index are evaluated once,
  before `e`.
- Empty lists: the type of `[]` comes from an annotation (`var xs: [int] =
  []`), a parameter or field type, or the other operand; otherwise it is
  check error E0309 (`cannot infer the type of an empty list; add an
  annotation like 'var xs: [int] = []'`). There is no `[int]()` form.
- A name may be declared once per block; inner blocks may shadow. A fn body
  is its own block, so it may shadow a parameter.

## Expressions (low to high precedence)

`a..b` (range), `or`, `and`, `not`, comparisons (`== != < <= > >=`, not
chainable), `+ -`, `* / %`, unary `-`, postfix (`f(x)`, `xs[i]`, `p.f`, `cap.method(x)`),
literals (`[1, 2]`, `Point { x: 1.0, y: 2.0 }` — any field order, every field
once; trailing commas allowed), comprehensions.

- `+ - * / %` need both operands of the same numeric type; `+` also
  concatenates `str`. Int `/` truncates toward zero.
- `== !=` work on int, float, bool, str (by content). `< <= > >=` on int,
  float, str (bytewise).
- `and`/`or` short-circuit and take/return bool.
- Indexing is bounds checked; negative indexes are errors.
- `a..b` (both int) is the half-open range. In a `for` head or a
  comprehension `for` clause it is iterated without allocating; anywhere else
  it is an `[int]` value (`print(0..3)` prints `[0, 1, 2]`).
- List comprehension: `[e for x in xs]`, `[e for x in xs if cond]`, and any
  sequence of `for` and `if` clauses, nested left to right like Python
  (`[a * b for a in xs for b in ys if a != b]`). Sources are lists or ranges;
  each iteration costs one step, like a loop. The result is a new list;
  `len([x for x in xs if p(x)])` counts matches. Clause variables are
  immutable and local to the comprehension.
- A struct literal is `Name { field: ...` or `Name {}`; there is no other use
  of braces.

## Builtins

`print(...)` (any values, space separated, newline; lists and structs print as
`[1, 2]` / `Point {x: 1.0, y: 2.0}` with strings quoted inside), `len(list|str)`,
`push(xs, v)`, `pop(xs)` (both change the list in place: `xs` is a `var`, an
`inout` parameter or a field/element of one), `str(x)`, `int(float|str|int)` (aborts on bad
input; see `parse_int` for the soft form),
`float(int|str|float)`, `sqrt(float) -> float`, `assert(cond, msg?)`. I/O is
done through capability methods (*Capabilities and I/O*), not builtins.

Option and Result builtins (none of them abort, except `unwrap`):

| Builtin | Behaviour |
|---|---|
| `parse_int(s: str) -> Option[int]` | `Some(n)` for strict decimal syntax (as `int(s)`), `None` otherwise (also out of range) |
| `parse_float(s: str) -> Option[float]` | same for floats (`1e5` exponents accepted; `inf`, `nan` and out-of-range values give `None`) |
| `find(xs: [T], v: T) -> Option[int]` | index of the first element equal to `v` (T is int, float, bool or str); one step per element scanned |
| `find(s: str, sub: str) -> Option[int]` | code-point index of the first occurrence of `sub` (`Some(0)` for `""`) |
| `get(xs: [T], i: int) -> Option[T]` | `Some(xs[i])`, or `None` when `i` is out of bounds (negative included) |
| `unwrap(x)` | the `Some`/`Ok` value; `None` / `Err(e)` is runtime error E0517 (`unwrap of None`, `unwrap of Err("bad")`) |
| `unwrap_or(x, default)` | the `Some`/`Ok` value, else `default` (always evaluated) |
| `is_some(o)`, `is_none(o)` / `is_ok(r)`, `is_err(r)` | variant tests (`bool`) |

Reductions (Python names):

| Builtin | Behaviour |
|---|---|
| `sum(xs: [int] or [float])` | sum; `0` / `0.0` for an empty list; int overflow is a runtime error |
| `min(xs)` / `max(xs)` | smallest / largest element of a `[int]`, `[float]` or `[str]`; empty list is runtime error E0516 (`min of empty list`) |
| `min(a, b)` / `max(a, b)` | of two values of the same type (int, float or str); ties return `a` |
| `any(xs: [bool])` / `all(xs: [bool])` | `false` / `true` for an empty list |

They charge one step per element. When the argument is a comprehension
(`sum([x * x for x in xs])`) no list is built: the reduction runs inside the
comprehension's loops (every element is still evaluated; `any`/`all` do not
stop early).

Text and ordering builtins:

| Builtin | Behaviour |
|---|---|
| `split(s: str, sep: str) -> [str]` | pieces between occurrences of `sep` (like Python `s.split(sep)`: `split("", ",")` is `[""]`); an empty `sep` splits into code points (`split("hé", "")` is `["h", "é"]`) |
| `join(xs: [str], sep: str) -> str` | concatenation with `sep` between elements; `""` for an empty list |
| `sort(xs: [int] or [float] or [str])` | in place (`xs` must be changeable, as for `push`), ascending, stable; floats with NaN last; strings bytewise |

Each charges one step per 64 bytes scanned or produced plus one per piece
(`split`, `join`), or one step per element per merge pass (`sort`, about
n log2 n). `sort`'s scratch buffer (8 bytes per element) is charged to
`--max-mem` while it lives.
Builtin names (and `Some`, `None`, `Ok`, `Err`, `Option`, `Result`) cannot be redefined.

## Errors

Lex and parse errors stop at the first error. The checker reports **all**
type and name errors at once, sorted by position:

```
check error at 6:13: undefined variable 'y'
  hint: did you mean 'x'?
```

A `hint:` line follows when there is an obvious fix: the closest name in
scope by edit distance (names of 3+ chars, distance at most a third of the
length), `add 'pub' to fn f in m.lim`, `declare it first: var z = ...`. Parse errors
hint at common slips from other languages: a `;` (`Limits has no semicolons`),
a `{` where a block should start (`end the line with ':' and indent the body`)
and unexpected indentation.

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
Type names may not be `int`, `float`, `bool`, `str`, `Option`, `Result`, `Env`, `Fs`, `Vars`, `Clock`, `Args` or `Stdin`, and a struct may not
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
| E0006 | lex | tab character (indent with 4 spaces) |
| E0007 | lex | bad indentation (not 4 spaces per level / unindent to no outer level) |
| E0100 | parse | syntax error (`expected ..., found ...`; includes `;`, a `{` block and an unexpected indent) |
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
| E0213 | check | missing program body (no top-level statements and no `fn main()`) |
| E0214 | check | bad `main` signature (must be `fn main()` or `fn main(env: Env)`) |
| E0215 | check | module file not found |
| E0216 | check | import cycle |
| E0217 | check | module name imported twice |
| E0218 | check | `break`/`continue` outside a loop |
| E0219 | check | capability method used without declaring its capability in `uses` |
| E0220 | check | unknown capability in `uses` |
| E0221 | check | module uses a capability the root file did not declare |
| E0222 | check | assignment to or change of a `let` value, a (non-inout) parameter or a loop variable; changing something that is not a variable, field or element |
| E0223 | check | top-level statements in an imported module |
| E0224 | check | top-level statements and `fn main` in the same file |
| E0225 | check | `inout` / `&` misuse: inout argument without `&`, `&` for a parameter that is not inout or outside a call argument, one variable passed to two inout parameters |
| E0226 | check | capability stored in a struct field, enum variant, list or Option/Result |
| E0227 | check | no such method (methods exist only on capabilities) |
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
| E0316 | check | cannot infer the type of `None` / `Ok(..)` / `Err(..)` (needs an annotation) |
| E0317 | check | non-exhaustive match (lists the missing variants) |
| E0318 | check | unreachable match arm |
| E0319 | check | bad pattern (unknown variant, wrong number of bindings, wrong literal type, unmatchable type) |
| E0320 | check | `?` on a non-Option/Result, or in a fn that does not return the same kind |
| E0321 | check | a match arm used as a value does not end with a value |
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
| E0515 | runtime | I/O failure (`cannot read file` / `cannot write file` / `cannot use directory`) |
| E0516 | runtime | `min`/`max` of an empty list |
| E0517 | runtime | `unwrap` of `None` or `Err` |

## Limits guarantee

By default the generated code:

- checks integer overflow (`__builtin_*_overflow`), bounds and division by zero;
- charges one step on every loop iteration (back edge, including each
  comprehension iteration and each element a reduction or range value visits)
  and every function call against `--max-steps` (default 1_000_000_000). The
  error position is the loop head expression (the comprehension source, the
  builtin call), or the called function's `fn` keyword;
- caps call depth at `--max-depth` (default 10000), reported at the called
  function's `fn` keyword. Infinite recursion therefore stops with an error
  instead of overflowing the C stack. The remaining C stack is also checked
  on every call (from `RLIMIT_STACK`, 512 KiB margin), so large frames hit the
  same error rather than a segfault;
- charges builtins doing O(n) work (string `+`, `==`, comparisons, `len(str)`,
  `str()`, `print`) one extra step per 64 bytes, and one step per value node
  formatted;
- charges every live heap byte against `--max-mem` (default 2048 MB; bytes
  are charged when allocated and credited when freed, so the budget bounds
  the memory in use, not the total ever allocated; this check also stays on
  with `--unchecked`). Formatting deeply nested values stops with an error
  (stack-checked in all modes).

The compiler caps syntax nesting (parentheses, indented and one-line blocks,
operator chains) at
3000 levels (`parse error ...: nesting too deep`).

`--unchecked` removes step, depth and overflow checks (integer arithmetic
wraps) for full speed. Bounds, division-by-zero and conversion checks remain.
Very deep recursion in unchecked mode can overflow the native stack.

## Migrating from v0.1

`python3 tools/migrate.py FILE.lim...` rewrites v0.1 files in place (`-` reads
stdin and prints). It converts braces to indented blocks, drops `;`, splits
one-line bodies, turns `else if` into `elif`, moves contracts into the body,
makes reassigned `let`s `var` (and adds `var p = p` for a reassigned
parameter), rewrites `x = x op e` to `x op= e` when the grouping is
unchanged, makes a final `return e` implicit, turns `fn main()` into
top-level statements (`--keep-main` keeps it) and renames user functions that
clash with the new builtins (`sum` -> `sum_`). It keeps comments. It does not
introduce comprehensions or reductions; that is a hand edit. Reassigned loop
variables are reported for a hand fix.

## Known limitations (v0.5)

- Evaluation order of function arguments, list-literal elements and the
  operands of a string `+` chain follows C
  and is unspecified, except when an operand changes a variable in place
  (then left to right, see *Values, mutation and memory*); struct literal
  fields are evaluated in declaration order only as far as C guarantees (also
  unspecified). Avoid side effects there.
- An inout argument is a variable or a field/element path of one; there is
  no way to keep a reference beyond the call. Changing one element of a
  value that is also held elsewhere copies the whole value once (e.g.
  `let row = grid[i]` kept alive while `grid[i][j]` is written).
- Option/Result payload values and parse results are heap allocated (one
  small block each); unit variants are not.
- Runtime locations pack the file index into the line (`line | file << 20`):
  files over 1,048,575 lines report wrong positions.
- A single stack frame larger than the 512 KiB margin (tens of thousands of
  live locals) could still overflow the stack before the check fires.
- Module-qualified names are only `alias.item` in call, struct-literal and type
  position; there are no re-exports, `import x as y`, or qualified globals.
- No conditional expression (`a if c else b`), no tuples, no dict/set
  comprehensions, no slices; `min`/`max` take one list or exactly two values.
- Sum types: no nested patterns, no `|` alternatives or guards in patterns,
  no `==` on sum values, no user-defined generics; a match value is only
  allowed as the whole right side of `let`/`var`/`=`/`return`. A `None` or
  `Ok(..)` with no expected type (e.g. the first element of a list literal)
  needs an annotation.
- Methods exist only on capabilities; there are no user-defined methods.
- No closures or function values, no maps, no float exponent literals.
  There is no string indexing; `split(s, "")` gives the code points as a list.
