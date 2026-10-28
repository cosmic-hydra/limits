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
| synthetic (`gen_large.py 1000`, v0.1 syntax) | 17,006 | 46-67 ms | 6,387 ms | 1,188 ms |

(`gen_large.py` now emits the v0.2 indentation syntax: the same 1000
functions in 15,008 lines; `limits check --time` on it reported 29-35 ms
frontend time over three runs.) The frontend checks the 17k-line program (`limits check`) in about 25 ms, roughly
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

## v0.4: values, reference counting and freeing

v0.4 replaced the never-freed bump arena with reference counting and value
semantics (copy-on-write; see SPEC.md, *Values, mutation and memory*). The
benchmark sources were migrated (`let` -> `var` where a value changes,
`inout` + `&` where a fn changed its caller's list or struct; nbody now
writes `bs[i].vx -= ...` instead of mutating an aliased `b`). All outputs are
unchanged. Measured on the same machine; "before" is the v0.3 compiler on the
v0.3 sources, built at the start of this stage.

`bench/run.sh` (best of 3, seconds; the run-to-run noise on this VM is about
±10%, sieve's checked number moved between 0.065 and 0.107 across runs of the
same binaries):

| bench | checked before | checked after | unchecked before | unchecked after | -O0 before | -O0 after |
|---|---|---|---|---|---|---|
| fib | 0.027 | 0.026 | 0.011 | 0.010 | 0.076 | 0.075 |
| loops | 0.156 | 0.156 | 0.120 | 0.124 | 1.157 | 1.210 |
| sieve | 0.065 | 0.085 | 0.069 | 0.067 | 0.340 | 0.390 |
| nbody | 0.377 | 0.445 | 0.338 | 0.432 | 1.851 | 2.987 |
| strings | 0.168 | 0.080 | 0.145 | 0.075 | 0.216 | 0.189 |

Because single best-of-3 runs are noisy, the old and new binaries were also
run interleaved, best of 7 (checked, ms):

| bench | before | after | after / before |
|---|---|---|---|
| fib | 25.5 | 25.0 | 0.98 |
| loops | 157.9 | 160.6 | 1.01 |
| sieve | 71.9 | 74.1 | 1.03 |
| nbody | 330.9 | 459.4 | 1.38 |
| strings | 159.3 | 74.0 | 0.46 |

- **strings is 2.1x faster**: `s = s + str(i) + ","` appends in place to the
  unshared string (linear instead of quadratic copying), and the 10^6
  temporary `str(i)` strings are freed and their blocks reused.
- **fib, loops, sieve** are unchanged within noise. Getting sieve there took
  making a list unique once before a loop that writes it (instead of a
  copy-on-write check per write), see SPEC.md.
- **nbody is 1.3-1.4x slower.** The v0.3 program held references to two
  bodies (`let b = bs[i]`) and changed them through those aliases; values
  cannot alias, so v0.4 writes `bs[i].vx -= ...`. The v0.3 compiler runs that
  indexed form in 0.39 s (8% over its aliased form), so the remaining cost is
  the copy-on-write check per written element: list elements that are
  structs are separately counted boxes, and each inner iteration must
  confirm `bs[i]` and `bs[j]` are unshared before writing them (removing all
  checks by hand gives 0.38-0.41 s). Consecutive writes through one path
  share one check and `bs` itself is checked once per call; hoisting the
  `bs[i]` check out of the inner loop by hand gave no measurable gain.
  Storing struct elements inline in the list would remove the per-element
  box; that is a larger representation change left for later.

Agent tasks (`bench/agent_tasks/measure.py`, best of 3): total Limits time
0.673 s before, 0.710 s after (Python 11.7 s); all 20 outputs still match
Python. Interleaved best of 7 per task (after / before): 01 0.90, 02 1.05,
03 0.96, 04 0.90, 05 0.65, 06 1.05, 07 0.99, 08 0.97, 09 0.98, 10 0.97,
11 1.10, 12 1.01, 13 0.60, 14 1.00, 15 0.99, 16 0.99, 17 1.05, 18 1.05,
19 1.00, 20 0.58. Three tasks got much faster because freed blocks are
reused (better cache locality than an ever-growing arena). Two layout
choices mattered: a string's bytes follow a 24-byte header (with a 32-byte
header the 300k short strings of 02_word_freq took 48-byte blocks and its
sort ran 50% slower), and memory still held when the program's top-level
code ends is left to the OS instead of being freed block by block (it is
freed, and checked, under `--leak-check`).

Source size: the migration added `var`, `inout` and `&`: the 20 agent tasks
went from 4837 to 4946 o200k tokens (+2.3%).

Compile time: generated C is larger (count updates, drop and copy functions
per type). The synthetic 15k-line program now takes 11.0 s in `cc -O2`
(was 6.1 s) and 1.8 s at `-O0` (was 1.2 s); the frontend takes about 120 ms
(was 56 ms). The small benchmarks compile in 130-320 ms as before.

## TinyCC for `limits run` / `limits test` / `--debug`

`limits run`, `limits test` and `--debug` now compile the generated C with
TinyCC (`tools/tcc/bin/tcc`, tcc 0.9.28rc mob@e23dabed, see
`tools/TINYCC.md`) when it is available; `limits build` (release) still uses
`cc -O2`. The C gets a small shim for `__builtin_{add,sub,mul}_overflow`
(`src/tcc_shim.h`) and is linked with `-lm`.

Wall time of the whole `limits run FILE` (release build of limits:
frontend + C compile + link + run), median of 10 runs, same machine as
above (4 cores, gcc 13.3.0). All three produce identical output.
`--cc tcc` / `--cc cc --debug` (`-O0 -g`) / `--cc cc --release` (`-O2`):

| program | tcc | gcc -O0 | gcc -O2 |
|---|---|---|---|
| examples/hello.lim | 16 ms | 224 ms | 210 ms |
| examples/fib.lim | 57 ms | 298 ms | 177 ms |
| examples/primes.lim | 68 ms | 282 ms | 328 ms |
| examples/shapes.lim | 22 ms | 309 ms | 496 ms |
| examples/structs.lim | 19 ms | 253 ms | 318 ms |
| examples/modules/main.lim | 19 ms | 269 ms | 287 ms |

The benchmark programs (median of 3, `--max-steps 100000000000`) show the
other side: tcc does not optimise, so compute-heavy programs run slower.

| program | tcc | gcc -O0 | gcc -O2 |
|---|---|---|---|
| bench/fib.lim | 149 ms | 308 ms | 173 ms |
| bench/loops.lim | 2871 ms | 1314 ms | 294 ms |
| bench/sieve.lim | 562 ms | 670 ms | 277 ms |
| bench/nbody.lim | 3773 ms | 3467 ms | 801 ms |
| bench/strings.lim | 253 ms | 395 ms | 346 ms |

The synthetic 15k-line program (`bench/gen_large.py`, one run each):
tcc 173 ms, gcc -O0 1808 ms, gcc -O2 9675 ms. Noisy single-machine timings.

If tcc rejects a program (it has fixed internal limits: an expression of
2500 chained `+` exhausts its value stack), limits rebuilds it with the
system compiler at `-O0`; `LIMITS_TCC_STRICT=1` disables that fallback.
In the test suite this happens only for that one test. `cargo test` passes
both with the default (tcc) and with `LIMITS_CC=cc` (gcc for everything).
