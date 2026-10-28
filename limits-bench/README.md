# Limits-Bench

Measures how well LLM agents write **Limits** on the first attempt, compared
with Python, on the same 60 small programming tasks. Every future language
change can then be judged by data instead of opinion:

- **pass@1**: share of tasks whose first submitted solution passes every test;
- **compile-error rate**: first attempts rejected by `limits build` with a
  lex, parse or check diagnostic;
- **runtime-error, timeout and wrong-answer rates**;
- **tokens per solved task**: mean size of passing solutions (tiktoken
  `cl100k_base` when importable, otherwise characters; `results.json` names the unit);
- **repair success per round**: of the tasks still failing before round k
  (k = 2, 3) that had a repair submitted, the share that pass after it.

## Layout

```
data/raw/HumanEval.jsonl.gz    openai/human-eval @ b9453e8f (sha256 b796127e…79ef)
data/raw/sanitized-mbpp.json   google-research/google-research mbpp/ @ db336cdc (sha256 ca95deaa…c8e9)
data/selection_log.txt         every task visited in dataset order and why it was kept or skipped
tools/build_tasks.py           raw data -> tasks/ (deterministic, fixed seed)
tasks/<id>.json                one task: description, I/O contract, tests
tasks/<id>.py                  official reference solution wrapped in the I/O contract
run.py                         the harness
solutions/limits-handwritten/  5 hand-written Limits solutions (harness self-test)
selftest/broken/, selftest/repair/   deliberately broken programs and a fake repair log
```

The raw files are 300 KB together, so they are committed (not gitignored).
Task ids are `he_NNN` (HumanEval/NNN) and `mbpp_NNN` (MBPP task_id NNN).

## Task format

Limits has no function-call test harness and its syntax will change, so each
task is language neutral: a whole program reads stdin and prints to stdout.

```json
{
  "id": "he_011",
  "source": "HumanEval/11",
  "description": "…natural language… + I/O contract + examples",
  "params": [{"name": "a", "type": "str"}, {"name": "b", "type": "str"}],
  "output_type": "str",
  "tests": [{"stdin": "111000\n101010\n", "stdout": "010010\n", "origin": "official"}, …]
}
```

`description` is the complete text an agent gets for the task. It is the
original docstring (HumanEval, with the `>>>` doctests removed and
`True/False/function` reworded to `true/false/program`) or the MBPP prompt
(`function` -> `program`), followed by the I/O contract and examples in plain
stdin/stdout form (2 for HumanEval; all official tests for MBPP, as in the
standard MBPP prompt format).

Encoding, one value after another, one line each unless stated:

| type | stdin / stdout form |
|---|---|
| int | decimal |
| bool | `true` / `false` |
| str | the string itself (never contains a newline; may be empty) |
| [int] | integers separated by single spaces; empty line for `[]` |
| [str] | a line with the count n, then n lines |
| [[int]] | a line with the count n, then n lines each in the [int] form |

Output comparison is exact except that `\r\n` is treated as `\n` and trailing
newlines are ignored (trailing spaces inside a line matter).

Tests (>= 5 per task, 323 total): `official` inputs are taken from the
dataset's own asserts; when fewer than 5 remain, `hand` inputs (4 tasks whose
inputs need structure, listed in `HAND_INPUTS`) or `generated` inputs (random,
seed 20261028, shaped like the official ones: ints in the observed range, lists
of observed lengths, strings over the observed alphabet) are added. Expected
outputs for **all** tests are produced by running the official reference
solution in python3. Counts: 225 official, 12 hand, 86 generated. Generated
inputs may fall outside an unstated precondition of the original task; the
reference's behaviour defines the answer there. This is a known source of
noise, identical for Limits and Python.

## Selection rule

Walk HumanEval in task order, then sanitized MBPP in task order, and keep a task
when all of these hold, until 30 are kept from each:

1. not in the documented manual `EXCLUDE` list in `tools/build_tasks.py`
   (only HumanEval/32 and /38 were reached: tolerance check on floats; input
   produced by an encoder in the prompt);
2. every assert calls the entry function with literal arguments and compares
   the result by `==`/`is` or truth (so `set(f(x)) == …`, tolerance checks and
   looped random tests, whose answer is not unique or not literal, are skipped);
3. every argument and every result has a type in {int, bool, str, [int], [str],
   [[int]]}, ints fit in 64 bits, strings have no newline (so floats, tuples,
   dicts, None and mixed lists are skipped: Limits has no maps or tuples, and
   float printing is not specified precisely enough to compare text);
4. the reference solution runs on every official input in python3.

Result: HumanEval/1–46 (30 kept) and MBPP/3–89 (30 kept); the full trace with
the reason for every skip is `data/selection_log.txt`. Rebuild with
`python3 -I tools/build_tasks.py`.

Fit with current Limits: stdin is available (`uses stdin;` + `input_line()`),
and `split`, `join`, `sort`, `int(str)` exist, so every task can be written.
Some tasks need characters or ASCII codes (case flipping, digit sums); Limits
has no `ord`/`chr` or string indexing, so they need `split(s, "")` and lookup
strings. That is a real cost of the language and is deliberately kept.

## Running

```
cd limits-bench
python3 run.py SOLDIR                       # SOLDIR/<id>.lim or <id>.py
python3 run.py tasks --lang python          # reference solutions (should be 100%)
python3 run.py SOLDIR --limits-bin PATH --cc /path/to/tcc --timeout 10 --only he_001,he_003
python3 run.py SOLDIR --repair-log          # SOLDIR/<id>.attempt1.lim, .attempt2.lim, .attempt3.lim
```

For `.lim` files the harness runs `limits build FILE -o TMP --json` once
(default binary `../native/target/release/limits`, `--limits-bin` overrides,
`--cc` is passed as `$CC`), then runs the binary once per test with the test's
stdin and a per-test timeout (default 10 s). `.py` files run as
`python3 -I FILE`. A task's status is `compile_error` (build exit 1 with a
lex/parse/check diagnostic), `toolchain_error` (C compiler failure or build
timeout; a harness problem, not the agent's), otherwise the worst test result:
`runtime_error` > `timeout` > `wrong_answer` > `pass`. A missing file is
`missing` and counts as a failure. Note a Limits infinite loop usually ends as
a `runtime_error` (E0507 step limit) before the timeout.

Output goes to `SOLDIR/_results/` (or `--out`): `results.json` (summary and
every attempt with status, tokens, diagnostics, per-test status and time),
`summary.md` (the table) and `feedback/<id>[.attemptN].txt`.

## Agent protocol

Run each task in a fresh agent context, once with Limits and once with Python.

1. The agent receives ONLY: the task `description` from `tasks/<id>.json` and
   the language reference text (for Limits: `native/SPEC.md` verbatim at the
   commit being measured; for Python: nothing extra). No other files, no
   repository access, no tools that execute code, no web.
2. It writes one complete solution file (`<id>.attempt1.lim` / `.py`). It may
   NOT compile, check or run anything before submitting attempt 1.
3. The harness grades it. If it failed, the agent is shown exactly
   `feedback/<id>.attempt1.txt` (status, compiler/runtime diagnostics, and for
   a failing test its stdin, expected and actual stdout) and may submit
   `<id>.attempt2.*`. Same once more for `<id>.attempt3.*`. No other runs are
   allowed between attempts. At most 2 repairs.
4. Grade with `python3 run.py SOLDIR --repair-log`. pass@1 and the error rates
   use attempt 1 only; repair success per round comes from attempts 2 and 3.
5. Record the model, its settings and the Limits commit with every result set.
   Token counts measure the solution file only (not the agent's reasoning).

## Self-test (2026-10-28)

- `python3 run.py tasks --lang python`: 60/60 pass. (The mean size printed
  there, 467 tokens, includes the shared I/O wrapper and is not a baseline.)
- `solutions/limits-handwritten/` (he_007, he_009, he_013, mbpp_003,
  mbpp_012): 5/5 pass with the current native binary.
- `selftest/broken/`: classified as parse error E0100 (he_013), check error
  E0219 (he_023), runtime error E0501 (he_024), wrong answer (mbpp_017),
  runtime error E0507 step limit (mbpp_059, an infinite loop) and lex error
  E0002 (mbpp_086), all as intended.
- `selftest/repair/` with `--repair-log --only he_007,he_009,he_013,mbpp_003`:
  round 2 success 1/3, round 3 success 1/2, final 3/4, as constructed.
