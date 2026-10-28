# limits benchmark results

All numbers below were measured on the machine described here with
`bench/run.sh` (run-time table, best of 3 wall-clock runs, build time excluded)
plus the ad-hoc commands noted where used. None are estimated. They come from
a shared cloud VM, so expect roughly ±10% noise between runs.

## Machine

- 4 vCPUs (`nproc` = 4), Intel Xeon @ 2.80GHz, 15 GiB RAM, Linux 6.18
- `cc (Ubuntu 13.3.0-6ubuntu2~24.04.1) 13.3.0` (gcc); clang is also installed
- Python 3.13.17, rustc 1.97.0
- limits flags: checked runs use `--max-steps 100000000000` (the default budget
  of 1e9 steps is too small for the 1e8-iteration loop plus everything else);
  the interpreter runs with `--max-steps 100000000000 --max-depth 100000`.

## Programs

| bench | what it does |
|---|---|
| fib | recursive `fib(32)` (calls + int arithmetic) |
| loops | nested `for` ranges, 1e8 iterations of `s = s + (i*j + j) % 7` |
| sieve | sieve of Eratosthenes to 1e7 on a `[bool]` built with 1e7 `push`es |
| nbody | 5-body simulation, 5e6 steps, structs + float math + `sqrt` |
| strings | 20 rounds of building a 2000-number string with `+`, then 1e6 `"item" + str(i)` pushed to a list |

Each has a `.py` twin; `fib` and `sieve` also have a `.c` ceiling, and `fib`,
`loops`, `sieve`, `strings` have a `.dyn.lim` version for the v0.1 interpreter
(`compiler/`). The interpreter has no structs, so nbody is skipped there.
`run.sh` compares every implementation's stdout with limits's; all matched.

## Run time (seconds, best of 3)

| bench | limits (checked) | limits --unchecked | limits --debug (-O0, checked) | C -O2 | python3 | v0.1 interpreter |
|---|---|---|---|---|---|---|
| fib | 0.033 | 0.011 | 0.113 | 0.014 | 0.360 | 3.475 |
| loops | 0.161 | 0.128 | 1.137 | - | 16.002 | 59.026 |
| sieve | 0.341 | 0.292 | 0.931 | 0.053 | 4.680 | 22.813 |
| nbody | 0.349 | 0.360 | 1.724 | - | 30.716 | - |
| strings | 0.175 | 0.156 | 0.223 | - | 0.428 | 1.205 |

Speedups of checked limits: about 11x (fib) to 100x (loops, nbody) over
CPython, and 70x to 370x over the v0.1 tree-walking interpreter. The strings
bench is allocation-bound and only about 2.4x faster than CPython.

## Effect of this stage's optimizations

Before and after on the same machine. "Before" is the code at the start of
this stage; "after" is the final table above. Both are best of 3 (before) or
best of 3 to 7 (spot checks), so small differences are noise.

| bench | checked before | checked after | unchecked before | unchecked after |
|---|---|---|---|---|
| fib | 0.028 | 0.025-0.033 | 0.011 | 0.008-0.011 |
| loops | 0.176 | 0.153-0.161 | 0.117 | 0.119-0.128 |
| sieve | 0.307 | 0.298-0.341 | 0.292 | 0.275-0.317 |
| nbody | **0.749** | **0.341-0.349** | 0.370 | 0.358-0.377 |
| strings | 0.311 | **0.175** | 0.313 | **0.156** |

What changed:

1. **Step counter in a register.** The step budget was a global that every
   loop iteration decremented. Any store through a pointer (struct field,
   list slot) may alias that global as far as C is concerned, so gcc reloaded
   and stored it on every iteration. Each function now copies the budget into
   a local `lm_ls`, decrements that, writes it back before each call and on
   return, and reloads it after each call. Counting is still exact (a new test
   checks the exact step at which a mixed loop/call program stops, and a test
   with 400 functions checks it across files). nbody, which stores struct
   fields in its inner loop, went from 0.75 s to 0.35 s and now costs the same
   as `--unchecked`.
2. **Cold, out-of-line error paths.** `lm_panic`, the bounds-error formatter
   and `assert` failure are `noreturn, cold, noinline`, and list growth is a
   separate `noinline` function. Before, the `snprintf` for the bounds error
   was inlined into every `xs[i]`. List get/set are now one compare plus one
   load or store.
3. **String building.** `a + b + c` on strings is one allocation and copy
   (`lm_concat(n, parts)`) instead of one per `+`, and `str(int)` formats
   straight into the arena without a temporary heap buffer. strings went from
   0.31 s to 0.17 s.

No guarantee was weakened: checked mode still checks overflow on every int
`+ - * / %` and unary minus, bounds, division by zero, the exact step count
and the call depth.

## Where the remaining gaps are

- **fib, checked vs unchecked (3x):** every call pays a step check plus the
  depth increment, decrement and check, and the `+`/`-` are overflow
  checked. Unchecked limits runs at C speed (0.011 vs 0.014 s).
- **sieve vs C:** fixed by packed lists (see the last section); it now runs
  within about 1.15x of C -O2, checked or unchecked.
- **loops:** gcc keeps both counters in registers. The checked overhead
  (about 25%) is the overflow checks on `*`, `+` and `%` plus one step per
  inner iteration.
- **strings:** still quadratic (immutable strings, `s = s + ...` copies the
  prefix) and the arena never frees, so this bench allocates about 180 MB.
  CPython is closer here because it appends in place when the string has a
  single reference.
- **-O0:** `--debug` binaries are 1.3x (strings) to 7x (loops) slower.

## Compile time

`limits build --time` splits the time into the frontend (lex, parse, check,
codegen, all in-process) and `cc`.

| program | lines | frontend | cc -O2 (default) | cc -O0 (`--debug`) |
|---|---|---|---|---|
| fib | 5 | 0.22 ms | 82 ms | 98 ms |
| loops | 9 | 0.14 ms | 77 ms | 92 ms |
| sieve | 16 | 0.15 ms | 96 ms | 94 ms |
| nbody | 90 | 0.61 ms | 200 ms | 136 ms |
| strings | 16 | 0.17 ms | 148 ms | 117 ms |
| synthetic (`gen_large.py 1000`) | 17,006 | 46-67 ms | 6,387 ms | 1,188 ms |

The frontend checks the 17k-line program (`limits check`) in about 25 ms, roughly
680k lines/s, so the C compiler dominates by 2 to 3 orders of magnitude.
For small programs, cc's time is mostly fixed cost (system headers and the
~300-line embedded runtime): about 80-100 ms at either optimization level, so
`-O0` gains nothing there. For large programs, `-O0` compiles 5.4x faster and
is the right edit-compile-run mode. `--debug` already used `-O0 -g`, and that
is now documented with these numbers in `SPEC.md`.

Changes in this stage for compile time:

1. **Parallel C compilation.** When the generated function code is over
   64 KiB, limits splits it into up to `nproc` C files, each with the shared
   runtime and declarations, compiles them in parallel and links them. For the
   17k-line program, single-file cc took **26.1 s at -O2 and 3.9 s at -O0**
   (measured before the change). Split into 4 files it takes **6.4 s and
   1.2 s**. Small programs stay in one file, so they keep whole-program
   inlining.
2. **Frontend:** tokens are moved out of the token vector instead of cloned,
   the lexer skips keyword lookup for identifiers that cannot be keywords and
   counts columns by bytes outside string literals, and the AST is not freed
   before exit. Together these took `limits check` on the large file from about
   36 ms to about 25 ms.

Why gcc is slow on the large file: its 1000 functions form a call chain, and
every function stays reachable. A variant where the functions are unreachable
compiles in 0.2 s because gcc drops them. The cost is about 25 ms per
reachable function at -O2 on this VM, measured on a 250-function single-file
version (6.6 s). It does not come from one pass. `-fno-inline` gave 4.2 s,
`-O1` gave 3.6 s, and turning off individual IPA passes (`-fno-ipa-cp`,
`-fno-ipa-modref`, `-fno-ipa-pure-const`, ...) changed little. The checked-mode panic branches add to it: the unchecked
build of the same file takes about half the cc time.

## Reproducing

```
native/bench/run.sh            # all tables except "before" (takes ~6 minutes)
python3 native/bench/gen_large.py 1000 > /tmp/large.lim
native/target/release/limits build /tmp/large.lim --time
```

## Re-check after the review fixes (stack check per call, step/memory charging in builtins)

Ad-hoc best-of-3 limits-only runs (same machine, run.sh's interpreter/Python columns not re-run):
fib 0.026 / 0.009 / 0.074, loops 0.165 / 0.115 / 1.185, sieve 0.285 / 0.265 / 0.877,
nbody 0.372 / 0.314 / 1.607, strings 0.166 / 0.154 / 0.215 (checked / unchecked / debug, seconds).
All within noise of the table above, so the table is left unchanged.

## Packed, typed lists

Lists used to store every element as an 8-byte `lm_val` union. Now the element
type picks the storage: `[bool]` 1 byte, `[int]`/`[float]` 8 bytes, `[str]`,
struct and nested lists one pointer. The runtime defines typed accessors per
storage type (`LM_LIST(b, bool)`, `(i, int64_t)`, `(f, double)`, `(p, void *)`:
`lm_get_b`, `lm_set_b`, `lm_push_b`, ...), and codegen picks one statically.
Bounds checks are unchanged: one unsigned compare against `len` with a cold
out-of-line error. `for x in list` already reads elements without a per-access
check (the loop compares `k < len` each iteration, which stays correct if the
body pushes), so no further hoisting was added.

Before/after, limits only, best of 5, same machine, run back to back with the
binary built from the previous commit (milliseconds, checked / unchecked):

| bench | before | after |
|---|---|---|
| fib | 24 / 12 | 25 / 8 |
| loops | 158 / 118 | 160 / 116 |
| sieve | **290 / 294** | **64 / 60** |
| nbody | 345 / 344 | 363 / 346 |
| strings | 193 / 156 | 168 / 146 |

The C -O2 sieve measured 65 ms in that session and 59 ms in the full `run.sh`
run below, so the sieve target (within 2x of C, unchecked) is met with room to
spare; the checked build costs about the same. The other benches do not
index lists in their hot loops and moved only within noise.

Full `bench/run.sh` after the change (best of 3, seconds):

| bench | limits (checked) | --unchecked | --debug | C -O2 | python3 | v0.1 interpreter |
|---|---|---|---|---|---|---|
| fib | 0.025 | 0.010 | 0.077 | 0.010 | 0.327 | 3.440 |
| loops | 0.165 | 0.125 | 1.135 | - | 16.228 | 57.989 |
| sieve | 0.068 | 0.069 | 0.351 | 0.059 | 4.536 | 23.846 |
| nbody | 0.410 | 0.443 | 1.524 | - | 31.463 | - |
| strings | 0.232 | 0.169 | 0.265 | - | 0.414 | 1.207 |

nbody and strings came out 10-30% slower in this one run than in the
back-to-back table above; neither bench's hot loop changed, so this is VM
noise (re-running the binaries alone gave the numbers in the first table).
Compile times in the same run were 100-270 ms for the small benches and
8.2 s / 1.8 s (-O2 / -O0) for the 17k-line program, versus 6.4 s / 1.2 s
earlier; not investigated further. The cc command line now carries `-Wall
-Wextra`, which may add a little.

Also in this stage: the generated C no longer needs `-w`. It compiles without
warnings under `-Wall -Wextra` (gcc 13; clang also reports none apart from the
pre-existing bracket-depth limit on 50000-deep nesting). Debug builds of
limits (so `cargo test`) add `-Werror` to enforce that. Runtime helpers are
marked `unused`, unused locals and parameters get `LM_U`, and conditions are
emitted as `if ((bool)(...))` to avoid clang's `-Wparentheses-equality`.
`-Wno-infinite-recursion` stays, since that warning is about the user's
program, not the generated code.

String building was not changed: the only real fix (in-place append when the
left operand is a uniquely owned local) needs ownership analysis and is not a
small change.
