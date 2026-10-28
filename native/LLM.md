# Limits for agents (v0.5)

Typed, Python-like, compiled. Grammar: `grammar.ebnf`. `limits run f.lim [-- args]`; `limits check f.lim` lists all errors with fix `hint:`s.

## Syntax
- Blocks: a line ending in `:`, then lines indented 4 more spaces (no tabs, braces, semicolons), or one statement after the `:`. Newlines are free inside `()[]{}`. `#` comments; `pass` is empty.
- Top-level statements are the program; declarations may come in any order. Top-level variables are invisible inside fns. Or use `fn main():` / `fn main(env: Env):` and no top-level statements.

## Types and values
`int` (64-bit, overflow checked), `float`, `bool`, `str` (UTF-8; `len` counts code points), `[T]`, structs, enums, `Option[T]`, `Result[T, E]`. No null, tuples, dicts, sets, closures, generics, slices, string indexing, ternary or user methods.
- No implicit conversions: `int(x)`, `float(x)`, `str(x)`; `1 + 2.0` is an error. Int `/` truncates; `%` has the sign of the left operand.
- Literals: `1_000`, `2.5` (no exponent), escapes `\n \t \\ \"`.
- `+` adds numbers and joins `str`; `== != < <= > >=` on int/float/str/bool (no chaining); `and or not`; `a..b` is a half-open int range.

## Variables and mutation
- Values never alias (`let b = a` copies). `let` is fully immutable (elements and fields too). `var` allows `x = e`, `x += e` (`-= *= /= %=`), `xs[i] = e`, `p.f = e`, `push(xs, v)`, `pop(xs)`, `sort(xs)`.
- Empty lists and `None`/`Ok`/`Err` need a known type: `var xs: [int] = []`, `var best: Option[int] = None`.
- Parameters are read-only (`var n = n` makes a copy). To change the caller's variable declare `p: inout T` and pass `&x`; `push/pop/sort` need no `&`.

## Functions, contracts, tests
```
fn sq(x: int) -> int: x * x  # last expression is the result
fn clamp(x: int, lo: int, hi: int) -> int:
    requires lo <= hi  # contract; also `ensures` (sees `result`)
    if x < lo: return lo
    min(x, hi)
test "clamp": assert(clamp(9, 0, 5) == 5, "upper bound")
```
Every path of a `-> T` fn must return. `limits test f.lim` runs tests.

## Control flow
`if`/`elif`/`else`, `while`, `for i in 0..n:`, `for x in xs:`, `break`, `continue`; conditions are `bool`. Loop variables are read-only copies (write `xs[i]`). Comprehensions: `[x * x for x in xs if x > 0]`.

## Structs, enums, match
```
struct Item:
    name: str
    qty: int
enum Shape:
    Circle(r: float)
    Rect(w: float, h: float)
fn area(s: Shape) -> float:
    match s:
        Circle(r): 3.14159 * r * r
        Rect(w, h): w * h
var it = Item { name: "pen", qty: 2 }
it.qty += 1
print(it, area(Rect(2.0, 3.0)))  # Item {name: "pen", qty: 3} 6.0
```
- Matches are exhaustive or end with `_:` (always for int/str). Patterns: `V(a, _)`, `V`, `3`, `-1`, `"s"`, `true`, `_`; no nesting or guards. Sum values have no `==`. A `_` after arms that already cover everything, and statements after `return`/`break`/`continue`, are errors (unreachable).
- `let v = match e:` (also `var`, `=`, `return`): each arm ends with a value or leaves (return/break/continue), and at least one arm gives a value.

## Option, Result, `?`
```
fn pair_sum(s: str) -> Option[int]:
    let p = split(s, ",")
    if len(p) != 2: return None
    Some(parse_int(p[0])? + parse_int(p[1])?)
fn age(n: int) -> Result[int, str]:
    if n < 0: return Err("negative: " + str(n))
    Ok(n)
print(pair_sum("2,40"), pair_sum("x"), age(-1))  # Some(42) None Err("negative: -1")
```
`e?` unwraps or returns `None`/`Err(e)` from a fn returning the same kind.

## Builtins
- `print(...)`, `len`, `push`, `pop`, `sort` (in place), `str`, `int(float|str)`, `float(int|str)` (bad text aborts), `sqrt`, `assert(c)`, `assert(c, msg)`.
- `split(s, sep)` (`""` gives code points), `join(xs, sep)`, `find(s, sub)`/`find(xs, v) -> Option[int]`, `get(xs, i) -> Option[T]`, `parse_int(s)`, `parse_float(s) -> Option` (accepts `1.5`, `-2`, `1e5`; `inf`/`nan` and out-of-range values give `None`).
- `unwrap` (aborts on None/Err), `unwrap_or(o, default)`, `is_some`, `is_none`, `is_ok`, `is_err`.
- `sum(xs)`, `min(xs)`, `max(xs)`, `min(a, b)`, `max(a, b)`, `any(xs)`, `all(xs)`.
- Missing: list `+` (push), slicing, substrings, `range()` (use `a..b`), `abs` (`max(x, -x)`), dicts (lists of structs, or parallel lists + `find`).

## Capabilities (I/O)
No global I/O. The program gets `env: Env` (predeclared at the root file's top level, or `fn main(env: Env)`), declares `uses`, and calls:
- `env.fs.read(path) -> str`, `fs.sub(dir) -> Fs` (`uses fs.read` or `fs.write`); `env.fs.write(path, data)` (`uses fs.write`)
- `env.vars.get(name) -> str` (`""` if unset; `uses env`), `env.clock.now_ms() -> int` (`uses time`)
- `env.args.list() -> [str]` (`uses args`), `env.stdin.read_line() -> str` (`""` at end; `uses stdin`)

Fns do I/O only via parameters typed `Fs`, `Vars`, `Clock`, `Args`, `Stdin` or `Env`. `fs.sub("data")` is an `Fs` limited to that directory (paths relative to it). Capabilities cannot be stored in structs/lists/Options or printed. Paths outside the working directory are denied.
```
uses fs.read, stdin
fn total(fs: Fs, name: str) -> int:
    sum([unwrap_or(parse_int(x), 0) for x in split(fs.read(name), "\n")])
print(total(env.fs.sub("data"), "nums.txt"), env.stdin.read_line())
```

## Modules
`import util.text` loads `util/text.lim`; use `text.f(x)`, `text.Item`. Only `pub` items are visible. Modules have no top-level statements and repeat the `uses` they need.

## More examples
```
fn is_prime(n: int) -> bool: n >= 2 and all([n % d != 0 for d in 2..n])
print([p for p in 0..20 if is_prime(p)])  # [2, 3, 5, 7, 11, 13, 17, 19]
```
```
fn add_all(xs: inout [int], k: int):
    for i in 0..len(xs):
        xs[i] += k
var a = [1, 2]
add_all(&a, 10)
push(a, 5)
print(a, join([str(x) for x in a], "-"))  # [11, 12, 5] 11-12-5
```
```
uses args
fn main(env: Env):
    let n = len([w for w in env.args.list() if w == "a"])
    let msg = match n:
        0: "none"
        _: str(n) + " found"
    print(msg)
```
```
var seen: [str] = []
for w in split("b a c a", " "):
    if is_none(find(seen, w)): push(seen, w)
sort(seen)
print(seen, len(seen))  # ["a", "b", "c"] 3
```
