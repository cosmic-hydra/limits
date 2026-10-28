# Python -> Limits translation corpus

Test-validated pairs of Python programs and idiomatic Limits translations, for
bootstrapping models on a language with no public training data. A pair is in
the corpus only if the compiled Limits program prints byte-identical stdout to
the Python program on every test input.

Written against the language described in `native/LLM.md` (the file is headed
v0.5) and checked with the compiler built from `native/` (`limits build`,
checked mode).

## Counts

| source | pairs | test cases | Python tok | Limits tok | Limits / Python |
|---|---:|---:|---:|---:|---:|
| `bench_*`: Limits-Bench reference solutions (`limits-bench/tasks/*.py`) | 60 | 323 | 28,175 | 4,939 | 0.18 (see note) |
| `agent_*`: `native/bench/agent_tasks/NN_*.py` | 20 | 20 | 3,543 | 5,153 | 1.45 |
| `app_*`: agent-style programs written for this corpus | 41 | 127 | 9,820 | 14,826 | 1.51 |
| **total** | **121** | **470** | 41,538 | 24,918 | |

Tokens are tiktoken `o200k_base` over every `.py` / `.lim` file of the pair
(module files included, `tests/` excluded). Median per-pair Limits/Python
ratio: agent 1.46, app 1.48.

Note on `bench_*`: the Python files are the official HumanEval/MBPP solutions
with their docstrings plus a ~40-line generic stdin/stdout harness, so the raw
ratio is not a like-for-like size comparison. With docstrings and the harness
removed, the 60 Python solutions are 3,226 tokens against 4,939 for Limits
(1.53), but the Limits side still contains its own input parsing.

All 121 pairs pass; no pair was rejected. Every `app_*` pair that reads input
(40 of 41; `app_shuffle` takes none) has at least 3 test inputs.
`bench_*` pairs use all of the task's tests (official + hand-written);
`agent_*` programs generate their own input, so they have one case each.

## How the pairs were made

- `bench_*`: translated from the Python reference solution and the task's I/O
  contract. 5 (`he_007`, `he_009`, `he_013`, `mbpp_003`, `mbpp_012`) reuse the
  existing hand-written solutions in `limits-bench/solutions/limits-handwritten/`;
  the other 55 were written for this corpus. All 60 passed on the first run.
  Translations are idiomatic rather than literal where Limits lacks the Python
  feature (no bitwise ops: `mbpp_006` counts differing bits by division,
  `mbpp_058` compares signs; no `ord`/`chr`: alphabet lookups with `find`;
  recursion in `mbpp_084` became a bottom-up table).
- `agent_*`: the existing `.lim` files beside the Python in
  `native/bench/agent_tasks/`, re-verified here.
- `app_*`: 41 small programs written for this corpus (Python first, then Limits).
  7 of them failed on the first try and were fixed; every failure and its fix
  is recorded in `failures.jsonl`:

| pair | first-attempt failure | fix |
|---|---|---|
| app_config | parse error: `match` as a call argument | bind the match with `let` first |
| app_config | user fn named `get` collides with the builtin | rename to `lookup` |
| app_temps | wrong float output (last digits of a mean) | Python 3.12+ `sum()` over floats is compensated; Python side now adds in a loop |
| app_text_module | `[int] + [int]` (list concatenation) | running max in a loop |
| app_bst | `?` on an Option inside a fn returning `bool` | rewrote the lookup as a loop over the Option child |
| app_dijkstra | type of `[]` / `None` inside a comprehension not inferred | bind a typed empty value first |
| app_date_diff | `requires` after `let` statements | `assert` on parsed fields |
| app_line_diff | `var i` redeclared in the same block | reassign instead |

## Feature coverage

Detected by regex over the `.lim` sources (comments and strings blanked);
see `FEATURES` in `verify.py`.

| feature | bench | agent | app | total |
|---|---:|---:|---:|---:|
| struct | 1 | 3 | 18 | 22 |
| enum | 0 | 0 | 7 | 7 |
| match | 3 | 1 | 25 | 29 |
| Option | 2 | 1 | 20 | 23 |
| Result | 0 | 0 | 9 | 9 |
| `?` operator | 0 | 0 | 7 | 7 |
| comprehension | 28 | 8 | 22 | 58 |
| inout | 1 | 17 | 6 | 24 |
| contract (`requires`/`ensures`) | 3 | 0 | 5 | 8 |
| inline `test` | 0 | 0 | 7 | 7 |
| modules (`import`) | 0 | 0 | 2 | 2 |
| `fn main(env: Env)` | 0 | 0 | 18 | 18 |
| stdin capability | 60 | 0 | 30 | 90 |
| args capability | 0 | 0 | 9 | 9 |
| fs.read | 0 | 0 | 9 | 9 |
| fs.write | 0 | 0 | 2 | 2 |
| fs.sub | 0 | 0 | 1 | 1 |
| env vars | 0 | 0 | 1 | 1 |
| while | 11 | 7 | 32 | 50 |
| float | 1 | 0 | 2 | 3 |

Thin spots worth growing next: modules, fs.write/fs.sub/env vars, and floats.
The clock capability is not covered (its output is not deterministic).

## Layout

```
pairs/<name>/prog.py        Python program (plus helper modules, e.g. util/stats.py)
pairs/<name>/prog.lim       Limits translation (plus imported modules, e.g. util/stats.lim)
pairs/<name>/tests/cases.json   [{"name", "stdin", "args", "files": {path: text}, "env"}]
pairs/<name>/tests/<case>.out   expected stdout
index.jsonl                 one row per pair: name, source, features, tokens_py,
                            tokens_lim, cases, status (+ reason when failing)
sources.json                origin of the bench_/agent_ pairs (others are "authored")
failures.jsonl              first-attempt failures and their fixes
fewshot.md                  8 short, diverse pairs for retrieval next to LLM.md
verify.py                   re-runs everything; make_fewshot.py rebuilds fewshot.md
```

## Verify / regenerate

```
cargo build --release --manifest-path native/Cargo.toml   # builds native/target/release/limits
pip install tiktoken                                        # for token counts
python3 corpus/verify.py            # all pairs; rewrites index.jsonl; exit 0 iff all pass (~80 s)
python3 corpus/verify.py app_bst    # selected pairs only
python3 corpus/make_fewshot.py      # rebuild fewshot.md from index.jsonl
LIMITS=/path/to/limits python3 corpus/verify.py   # use another compiler build
```

For each case `verify.py` makes a fresh temp directory with the case's
`files`, runs `python3 prog.py ARGS` and the compiled Limits binary with the
case's stdin and env, and requires both outputs to equal `<case>.out`. Pairs
with inline `test` blocks must also pass `limits test`. For `bench_*` pairs
the `.out` files come from the task JSON, so the Python reference is checked
too. To add a pair, create `pairs/<name>/` with `prog.py`, `prog.lim` and
`tests/cases.json`, then run `python3 verify.py --expect <name>` to record
the Python output as expected (refused for `bench_*`) and check the Limits side.
