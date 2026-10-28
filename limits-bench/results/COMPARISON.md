# Limits-Bench: baseline-v1 vs redesign-v2

Same 60 tasks (30 HumanEval-style `he_*`, 30 MBPP-style `mbpp_*`), one agent
attempt per task, graded with `run.py --repair-log` (Limits) and
`run.py --lang python` (Python). Token counts: tiktoken `cl100k_base`.
Raw data: `baseline-v1/` and `redesign-v2/` (`summary.md`, per-run `results.json`,
`solutions/`).

## Headline

| metric | baseline-v1 Limits | redesign-v2 Limits | baseline-v1 Python | redesign-v2 Python |
|---|---|---|---|---|
| pass@1 | 100.0% (60/60) | 100.0% (60/60) | 100.0% (60/60) | 100.0% (60/60) |
| pass after repairs | 100.0% | 100.0% | n/a | n/a |
| repair rounds used | 0 | 0 | n/a | n/a |
| compile-error rate (lex/parse/check) | 0.0% | 0.0% | 0.0% | 0.0% |
| runtime-error rate | 0.0% | 0.0% | 0.0% | 0.0% |
| timeout rate | 0.0% | 0.0% | 0.0% | 0.0% |
| wrong-answer rate | 0.0% | 0.0% | 0.0% | 0.0% |
| mean tokens, passing solutions | 136.0 | 78.0 | 83.4 | 61.2 |

## Tokens: Limits vs Python on the same tasks

All 60 tasks pass in both languages in both runs, so every mean is over the same 60 tasks.

| | baseline-v1 | redesign-v2 |
|---|---|---|
| Limits / Python (ratio of means) | 1.63x | 1.27x |
| median per-task Limits/Python ratio | 1.66x | 1.25x |
| tasks where Limits <= Python tokens | 13 / 60 | 17 / 60 |
| he_* mean (Limits / Python) | 144.2 / 90.3 | 80.0 / 50.5 |
| mbpp_* mean (Limits / Python) | 127.8 / 76.4 | 76.0 / 72.0 |

Change v1 to v2: Limits got 58.0 tokens shorter per task on average (-43%), and
was shorter on all 60 tasks. Python also got 22.1 tokens shorter per task (-27%),
even though the Python language did not change. That points to a large solver
and spec effect (see confounders below).

## Top errors

None. Neither run produced any compile, runtime, timeout or wrong-answer
failure on the first attempt, so there is no error distribution to rank and the
repair loop was never exercised. Both runs hit the ceiling on correctness.

Side observation: the baseline-v1 Limits solutions no longer compile under the
current (redesigned) compiler (0/60 pass, 60/60 compile errors when regraded).
This is expected, since the redesign changed the surface syntax. It does mean
the v1 Limits numbers above come from the stored `baseline-v1/summary.md`, not from a
regrade. The v1 Limits token counts were re-measured from the stored solutions
and match the stored 136.0.

## Confounders (read before drawing conclusions)

- **Ceiling effect.** pass@1 is 100% in every cell. The benchmark cannot tell the
  two designs apart on correctness. Harder tasks, or tasks that stress the
  redesigned features, are needed to measure that.
- **Small N = 60.** Even at 100%, a 95% interval on a binomial rate with 60/60
  passes still reaches down to about 95% (rule of three). The token differences are large
  relative to that, but they come from one sample per task, with no repeated
  sampling and no variance estimate.
- **Different solver agents.** v1 and v2 were written by different agent
  instances (groups g0/g1/g2 each time, different sessions). Per-task style, and
  verbosity in particular, varies between agents. The 27% drop in *Python* tokens, where the
  language is unchanged, shows how big this effect can be. A cautious reading credits the
  redesign only with the narrowing of the Limits/Python *ratio* (1.63x to 1.27x),
  not with the raw 43% drop.
- **Spec format change.** The task/spec presentation given to agents changed
  with the redesign. That affects both languages and is not separable from the
  solver effect here.
- **Same model family.** All solvers are the same model family that designed
  Limits and wrote its docs. Its fluency may not transfer to other agents, and
  familiarity bias favours the language.
- **Python is a solved baseline for these tasks.** HumanEval and MBPP appear
  heavily in training data, which inflates Python's advantage in brevity and
  correctness compared with a new language.

## Bottom line

On correctness both versions saturate the benchmark, so there is no measurable
difference. On size, redesign-v2 Limits solutions are much shorter (78.0 vs 136.0 tokens),
and the overhead relative to Python on the same tasks fell from 1.63x to 1.27x.
Part of that gain is attributable to the solver and the spec rather than the
language, because Python shrank too. The next useful step is a harder task set
with several samples per task and a fixed solver configuration.
