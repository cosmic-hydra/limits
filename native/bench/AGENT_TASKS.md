# Agent tasks: Limits vs Python 3

Twenty small, realistic programs of the kind an agent writes on the fly, each
written idiomatically in Limits (`bench/agent_tasks/NN_name.lim`) and in
Python 3 (`NN_name.py`). Every program generates its own input with the same
31-bit LCG, so no I/O capability is needed and both versions must print
byte-identical output. `bench/agent_tasks/measure.py` builds each Limits
program (release compiler, checked mode), runs both, fails on any output
difference, and prints the table below.

```
python3 bench/agent_tasks/measure.py          # from native/
```

All numbers were measured, not estimated: 4 vCPU Xeon @ 2.80GHz cloud VM,
gcc 13.3, Python 3.13.17. Run times are best of 3 wall clock, Limits build
time excluded, checked mode (`--max-steps 100000000000`; default bounds,
overflow, depth and memory checks all on). Expect roughly ±10-20% noise on
the short Limits times; a second run gave totals of 0.71 s to 0.88 s.

## Size measure

- **lines**: non-blank lines that are not `#` comments.
- **chars**: characters of those lines (indentation included, trailing
  whitespace removed, one `\n` each).
- **tokens**: the same text through `tiktoken` (`pip install tiktoken`
  worked here) with `o200k_base` and `cl100k_base`. The two encodings
  differ by at most a few tokens per file, so the analysis uses o200k.

## v0.2 syntax redesign (indentation, script mode, comprehensions)

The task files are now in the v0.2 syntax (SPEC.md). They were converted with
`tools/migrate.py` and then hand-edited where a comprehension or one of the new
`sum`/`min`/`max` builtins is the natural way to write the code
(02, 03, 04, 05, 07, 10-15, 18, 19). Same machine, same session; "v0.1" is the
previous file built with the previous compiler, "migrated" is the converter's
output before the hand edits, "v0.2" is the committed file. Both versions
print byte-identical output. Run time is best of 5 (s), checked mode.

| task | v0.1 lines | v0.2 lines | v0.1 tok | migrated tok | v0.2 tok | Python tok | v0.1 s | v0.2 s |
|---|---|---|---|---|---|---|---|---|
| 01_csv_aggregate | 33 | 27 | 383 | 347 | 347 | 243 | 0.080 | 0.084 |
| 02_word_freq | 39 | 25 | 339 | 296 | 278 | 169 | 0.104 | 0.094 |
| 03_dedupe_sort | 23 | 11 | 194 | 168 | 144 | 89 | 0.035 | 0.033 |
| 04_matmul | 36 | 18 | 267 | 231 | 194 | 141 | 0.011 | 0.011 |
| 05_json_format | 30 | 25 | 322 | 301 | 291 | 192 | 0.039 | 0.036 |
| 06_binary_search | 37 | 28 | 285 | 252 | 250 | 154 | 0.025 | 0.025 |
| 07_bfs_grid | 39 | 28 | 351 | 317 | 307 | 255 | 0.008 | 0.008 |
| 08_lru_cache | 33 | 24 | 259 | 229 | 229 | 148 | 0.018 | 0.019 |
| 09_tokenizer | 70 | 55 | 579 | 493 | 493 | 281 | 0.053 | 0.053 |
| 10_top_k | 18 | 10 | 146 | 128 | 115 | 77 | 0.037 | 0.038 |
| 11_rle | 31 | 20 | 268 | 237 | 224 | 137 | 0.022 | 0.023 |
| 12_prime_sieve | 23 | 15 | 145 | 117 | 108 | 84 | 0.012 | 0.012 |
| 13_levenshtein | 49 | 26 | 402 | 368 | 299 | 198 | 0.018 | 0.018 |
| 14_interval_merge | 43 | 30 | 308 | 268 | 255 | 156 | 0.023 | 0.023 |
| 15_histogram | 26 | 15 | 283 | 251 | 186 | 138 | 0.008 | 0.007 |
| 16_collatz_max | 21 | 15 | 123 | 101 | 101 | 88 | 0.086 | 0.088 |
| 17_fizzbuzz | 23 | 16 | 146 | 117 | 117 | 85 | 0.031 | 0.032 |
| 18_bank_ledger | 34 | 23 | 293 | 255 | 230 | 210 | 0.009 | 0.009 |
| 19_moving_average | 31 | 19 | 218 | 183 | 171 | 125 | 0.008 | 0.008 |
| 20_expr_eval | 76 | 57 | 575 | 521 | 521 | 371 | 0.062 | 0.060 |
| **total** | 715 | 487 | 5886 | 5180 | 4860 | 3341 | 0.690 | 0.682 |

o200k tokens: 5886 -> 5180 after mechanical migration (-12.0%) -> 4860 after
the hand edits (-17.4%); cl100k: 5888 -> 4861 (-17.4%). Lines: 715 -> 487
(-31.9%). Limits is now 1.45x Python's tokens on these tasks (was 1.76x).
Total run time did not change (0.690 s vs 0.682 s, within noise); a
reduction over a comprehension (`sum([... for k in 0..n])`) compiles to one
fused loop, which kept 04_matmul at 11 ms (it was 44 ms when the
comprehension built a list per element).

The same converter applied to the 60 first-attempt agent solutions of
Limits-Bench baseline-v1 (copies, outside this tree): all 60 still pass the
benchmark tests after conversion, and their mean size went from 136.0 to
122.1 cl100k tokens (-10.2%; mechanical only, no hand edits). One solution
defined its own `fn sum`, which the converter renamed to `sum_`.

The tables below are the v0.1 measurements, kept for history.
`features.py` estimates rewrites of the v0.1 brace syntax and no longer
applies to the current files; the `before/` files were migrated too, so
`measure.py` can still build them.

## v0.1 results (after the `split` / `join` / `sort` builtins)

| task | Limits lines | Limits chars | Limits tok o200k | Limits tok cl100k | Py lines | Py chars | Py tok o200k | Py tok cl100k | Limits tok before | Limits s | Python s | speedup |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 01_csv_aggregate | 33 | 1029 | 383 | 382 | 24 | 687 | 243 | 242 | n/a | 0.112 | 0.668 | 5.9x |
| 02_word_freq | 39 | 1035 | 339 | 339 | 15 | 513 | 169 | 168 | n/a | 0.126 | 0.337 | 2.7x |
| 03_dedupe_sort | 23 | 487 | 194 | 194 | 8 | 210 | 89 | 89 | 425 | 0.034 | 0.215 | 6.3x |
| 04_matmul | 36 | 801 | 267 | 267 | 11 | 364 | 141 | 141 | 267 | 0.010 | 0.218 | 21.4x |
| 05_json_format | 30 | 933 | 322 | 322 | 23 | 566 | 192 | 193 | 355 | 0.043 | 0.537 | 12.5x |
| 06_binary_search | 37 | 802 | 285 | 285 | 19 | 392 | 154 | 154 | 285 | 0.026 | 0.299 | 11.7x |
| 07_bfs_grid | 39 | 1012 | 351 | 351 | 24 | 651 | 255 | 255 | 351 | 0.009 | 0.400 | 44.8x |
| 08_lru_cache | 33 | 767 | 259 | 259 | 20 | 452 | 148 | 146 | 259 | 0.016 | 0.154 | 9.5x |
| 09_tokenizer | 70 | 1849 | 579 | 578 | 27 | 745 | 281 | 280 | n/a | 0.058 | 0.340 | 5.9x |
| 10_top_k | 18 | 388 | 146 | 146 | 8 | 194 | 77 | 77 | 377 | 0.043 | 0.221 | 5.1x |
| 11_rle | 31 | 778 | 268 | 268 | 14 | 383 | 137 | 137 | n/a | 0.029 | 0.121 | 4.2x |
| 12_prime_sieve | 23 | 474 | 145 | 145 | 8 | 228 | 84 | 83 | 145 | 0.013 | 0.490 | 38.0x |
| 13_levenshtein | 49 | 1181 | 402 | 402 | 21 | 543 | 198 | 198 | n/a | 0.018 | 0.155 | 8.7x |
| 14_interval_merge | 43 | 1016 | 308 | 308 | 17 | 423 | 156 | 157 | 539 | 0.030 | 0.642 | 21.6x |
| 15_histogram | 26 | 681 | 283 | 282 | 12 | 327 | 138 | 137 | 283 | 0.008 | 0.376 | 49.0x |
| 16_collatz_max | 21 | 421 | 123 | 123 | 9 | 223 | 88 | 88 | 123 | 0.102 | 7.429 | 72.8x |
| 17_fizzbuzz | 23 | 478 | 146 | 146 | 7 | 232 | 85 | 85 | 168 | 0.035 | 0.410 | 11.7x |
| 18_bank_ledger | 34 | 929 | 293 | 294 | 26 | 613 | 210 | 210 | 293 | 0.009 | 0.539 | 57.1x |
| 19_moving_average | 31 | 662 | 218 | 218 | 13 | 306 | 125 | 125 | 218 | 0.008 | 0.391 | 47.8x |
| 20_expr_eval | 76 | 1796 | 575 | 579 | 44 | 1203 | 371 | 379 | n/a | 0.058 | 0.326 | 5.6x |
| **total** | 715 | 17519 | 5886 | 5888 | 350 | 9255 | 3341 | 3344 | | 0.787 | 14.268 | 18.1x |

"Limits tok before" is the o200k count of the version written without the
new builtins (`before/NN_name.lim` where it differs, the same file where it
does not use them, `n/a` where the task could not be written at all; see
below).

## Honest reading

- **Correctness**: all 20 pairs print identical output (checked by the
  script on every run). All 14 Limits programs that existed before the new
  builtins also passed on the first compile; the new ones did too.
- **Speed**: checked native Limits is faster than CPython on every task,
  18x in total (0.79 s vs 14.3 s), ranging from 2.7x (word frequency:
  CPython's `Counter` and `str.split` are C code, and the Limits version
  sorts 300k strings) to 73x (Collatz: pure integer loops). Text-heavy tasks
  (01, 02, 09, 11, 20) gain 3-9x; numeric/array tasks 10-70x.
- **Tokens**: Limits needs **more tokens than Python on all 20 tasks**:
  5886 vs 3341 o200k tokens in total (1.76x), 715 vs 350 lines (2.0x). The
  smallest gap is bank ledger (1.4x: both sides are mostly explicit
  control flow over structs); the largest are top-k, dedupe-sort, word
  frequency and the sieve (1.7-2.2x), where Python has a one-call library
  answer (`heapq.nlargest`, `sorted(set(...))`, `Counter`, slice
  assignment).

## Where the extra tokens come from

Ranked by measured savings. For the syntax items, each row applies the
rewrite mechanically to the 20 Limits files (regexes in
`agent_tasks/features.py`) and re-counts o200k tokens, so the numbers
are real counts for these files, not guesses; they are not additive in
every combination.

| rank | missing feature | sites | o200k tokens saved | share of Limits total | cost to add |
|---|---|---|---|---|---|
| 1 | indentation-delimited blocks (no `{` / `}` lines) | 344 | 321 | 5.5% | large: lexer/parser redesign |
| 2 | module-level state (globals): the RNG seed list threaded through every call | 86 | 269 | 4.6% | medium: globals + init order |
| 3 | no `let` keyword (assignment declares) | 161 | 161 | 2.7% | small parser change, but loses the typo check that `let` gives (`x = 1` to an undeclared name is error E0202 today), so not recommended |
| 4 | list comprehension `[e for v in r]` | 11 | 130 | 2.2% | medium |
| 5 | inferring an empty list's type from its first use (`let xs = [];`) | 31 | 124 | 2.1% | medium (checker needs deferred inference) |
| 6 | compound assignment `x += e` | 48 | 107 | 1.8% | small (~20 lines of parser desugaring) |
| 7 | negative index / `last(xs)` (`xs[len(xs) - 1]`) | 9 | 39 | 0.7% | small, but negative indexes are a bounds error today on purpose |
| 8 | `min` / `max` builtins | 4+ | 18+ | 0.3%+ | small (only the simplest `if a > b { b = a; }` shape was matched) |

Not measured mechanically (library-level gaps, visible per task):

- **maps / dicts**: word frequency (02) and CSV aggregation (01) replace
  `Counter`/`dict` with sort-and-count or a linear scan of a key list.
- **`sum`, `max`, `min` over a list, string repeat (`"#" * n`), list
  slices (`xs[:8]`)**: histogram (15), moving average (19), RLE (11),
  interval merge (14) spell these as 3-5 line loops.
- **sort by key**: interval merge packs `(start, len)` into one int to sort.
- **regex / character classes**: the tokenizer (09) is 70 lines vs 27.

## Fixes implemented in this stage

Before this stage Limits had **no way to read characters or fields out of a
string** (no indexing, no split) and no sort. Six of the 20 tasks (CSV
parsing, word frequency, tokenizer, RLE, Levenshtein, expression
evaluator) could not be written at all, and every task needing order had to
carry a 33-line hand-written merge sort. The three cheapest fixes with the
largest effect, all builtins (about 45 lines of Rust in `check.rs`/`cgen.rs`
plus about 60 lines of C in `runtime.h`; see SPEC.md *Builtins*):

1. `split(s, sep) -> [str]` (empty `sep` = code points, which doubles as
   character access);
2. `join(xs, sep) -> str`;
3. `sort(xs)` for `[int]`, `[float]`, `[str]`: stable merge sort, steps
   charged per element per pass, scratch buffer charged to `--max-mem`.

These rank above the syntax items in the table because they are the only
fixes that change what can be expressed, and each is small. All existing
budgets apply to them (step limit, memory limit, located runtime errors).

Before / after, o200k tokens and run time (best of 3, s):

| task | tokens before | tokens after | Python tokens | s before | s after |
|---|---|---|---|---|---|
| 03_dedupe_sort | 425 | 194 | 89 | 0.070 | 0.039 |
| 05_json_format | 355 | 322 | 192 | 0.035 | 0.039 |
| 10_top_k | 377 | 146 | 77 | 0.061 | 0.035 |
| 14_interval_merge | 539 | 308 | 156 | 0.031 | 0.025 |
| 17_fizzbuzz | 168 | 146 | 85 | 0.026 | 0.038 |
| 01, 02, 09, 11, 13, 20 | not expressible | 2546 | 1399 | - | see table |

On the 14 tasks expressible before, Limits went from 4088 to 3340 tokens
(Python: 1942), i.e. from 2.1x to 1.7x Python. Fizzbuzz and JSON got
slightly slower after: `join` builds the whole 3-4 MB output string, while
the before versions only summed lengths (the outputs are equal).

## Next cheapest wins

From the table: compound assignment (rank 6, ~20 lines) and `min`/`max`/
`sum` builtins are each well under 150 lines; a map type and globals would
remove the two biggest structural gaps (01/02 and the RNG threading) but are
larger than this stage's budget.

## v0.3 sum types

Only one task changed: 08_lru_cache now uses `match find(cache, k):` with
`Some(at)` / `None` arms instead of a `var at = -1` search loop and an
`at >= 0` test. Measured with `measure.py --no-time`: 24 -> 22 lines, 229 ->
206 o200k tokens (Python: 146); output unchanged (all 20 tasks still match
Python). The other tasks have no absent-value sentinel where an Option is
shorter, so they compile unchanged. The new `examples/shapes.lim` shows
enums, `Result` and `?` together.
