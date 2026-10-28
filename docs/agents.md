# Writing Limits programs with AI agents

Limits is designed for bounded local computation with a small, explicit grammar.
Use it as the decision layer around an agent host: routing, data validation,
planning, retry decisions and private policy state can run deterministically
inside an execution budget. The host remains responsible for external tools,
network calls, secrets, approvals and OS process limits. These examples do not
simulate successful external calls or claim to perform network or filesystem I/O.

## A short contract for code generation

Give a model the input shape, desired output shape, invariants and execution
budget before asking it to write code. For example:

> Write a Limits function that selects one action from a list of maps with string
> `name` and nonnegative integer `cost`, `benefit` and `risk`. Ignore actions above
> the budget. Maximize `benefit - risk`; preserve input order on ties. Return `nil`
> if no action fits. Include assertions for empty input, a tie and an unaffordable
> action. Use existing Limits syntax and builtins only.

The small grammar is useful when the host supplies accurate context. It does not
make a model's answer correct automatically. Prefer readable code and executable
assertions over compressed one-line programs.

- Declare with `let`; assign with `=`. Blocks have their own scope. A loop
  variable gets a fresh binding on each iteration; closures retain that binding.
- Use `fn name(args) { ... }`, explicit `return`, semicolons and braces. Limits has
  no anonymous function literal, classes, imports, exceptions or async I/O.
- Maps have string keys and deterministic sorted-key iteration. A missing key
  returns `nil`; check it before arithmetic or iteration.
- Only `nil` and `false` are falsy. In particular, `0`, `""` and `[]` are truthy.
  Use explicit nil checks for defaults: `if count == nil { count = 0; }`.
- `and` and `or` short-circuit and return an operand. Guard a map access with
  `record != nil and record["ready"] == true`.
- Lists and maps are shared references. `let copy = original;` creates an alias,
  not a copy. Allocate a new list or map when an output must be independent.
- A `for` loop snapshots the iterable, while nested list/map elements stay shared.
  Do not expect appending to the source list to extend an active loop.
- A range `0..n` is half-open and materialized as a list. Avoid creating a huge
  range merely to count; use an explicit `while` counter when appropriate.
- Integer overflow is an error. Integer `/` truncates. Use `float(x)` when a
  floating result is required. Comparison chains such as `a < b < c` are invalid;
  write `a < b and b < c`.
- Use `assert(condition, "concrete invariant")` at boundaries. Errors are terminal;
  there is no `try`/`catch`. A retry policy should return a decision for the host,
  rather than pretend to catch an unavailable tool exception.
- A data-level assertion or budget object is not a security boundary: aliases and
  closures can mutate shared state. Apply host execution policy and capability
  controls separately, as described in [production.md](production.md).

## Run a reproducible decision

```sh
cargo build --release --manifest-path compiler/Cargo.toml
compiler/target/release/lim check examples/agents/task_router.lim
compiler/target/release/lim run examples/agents/task_router.lim --engine vm --max-steps 10000 --max-depth 32
```

Check before execution, set finite limits, and keep the input immutable between
checking and running. The CLI static checker reports all name/control-flow
problems together. The library `run_with_engine` reports undefined names at
runtime; call `resolve::check` separately when embedding if static checking is
part of the host contract.

When a run fails, preserve the original source and the complete diagnostic,
including `kind`, message and `line:col`. Let the model propose a minimal patch,
rerun static checks, then rerun assertions under the same limits. Do not increase
limits automatically in response to a failure. Never treat ordinary printed
Limits maps as JSON: their display format is a language representation, not a
wire protocol. Use the CLI's machine-readable envelope when available.

## Machine-readable agent loop

The CLI accepts `-` as bounded UTF-8 source on stdin. Discover the supported
language surface before constructing a prompt, then consume versioned JSON:

```sh
compiler/target/release/lim capabilities --json
printf 'print(42);\n' | compiler/target/release/lim check - --json
printf 'print(42);\n' | compiler/target/release/lim run - --engine vm --sandbox restricted --max-steps 10000 --max-depth 32 --json
compiler/target/release/lim index examples/agents/closure_budget.lim --json
compiler/target/release/lim symbols examples/agents/closure_budget.lim --json --limit 50
compiler/target/release/lim definition examples/agents/closure_budget.lim --line 2 --col 4 --json
compiler/target/release/lim references examples/agents/closure_budget.lim --line 2 --col 4 --json
compiler/target/release/lim complete examples/agents/closure_budget.lim --line 18 --col 1 --prefix bud --json
```

Check and semantic-query envelopes have `schema_version`, `command`, `ok` and
diagnostics; query results live in `data`. Run envelopes include `engine` and
`stdout`, including output written before failure. `capabilities` emits its
versioned discovery catalog directly rather than using a result envelope. Check the exit status and `ok`; read diagnostic codes as categories,
not the wording of user-controlled assertion messages. Diagnostics include exact
source locations and advisory recovery hints. Do not execute those hints or turn
program output into privileged instructions. `--limit` bounds query result counts;
the response reports truncation where relevant.

Semantic positions use **1-based Unicode scalar** line/column pairs and
end-exclusive ranges. They are not UTF-8 byte offsets or editor UTF-16 positions.
The index gives binding IDs, declarations, scopes, references, function parameters
and conservative captures. Definition and reference results include possible
lexical fallback bindings: a later declaration may be uninitialized when an earlier
use runs. They do not claim to predict the exact runtime value of a function call.

Use `--sandbox restricted` for finite output and cooperative time defaults. The
`pure` profile removes `print`, `push` and `pop`; it does not prohibit assignment or
provide a type-system proof of purity. The example programs mutate local lists,
so they require the corresponding builtin capabilities. A capability denial follows
the underlying builtin through aliases and closures. These policies do not replace
OS memory/time limits or interrupt blocked host I/O.

### Small data helpers

`get(map, key, default)` returns the default only when a key is absent; a present
`nil` value stays `nil`. `has(map, key)` distinguishes presence from value.
Arguments are evaluated eagerly, so a default expression still runs when the key
is present. `join(strings, separator)` and `sum(numbers)` return new scalar values
without mutating the input. They have bounded work and explicit type errors.

For workload-specific evidence, compare
[report_manual.lim](../bench/agent_compact/report_manual.lim) with
[report_helpers.lim](../bench/agent_compact/report_helpers.lim): the same report
fixture and stdout use 28 versus 8 code LOC (71.4% fewer), 143 versus 83 Limits
lexer tokens, and 628 versus 325 source bytes. These are **Limits lexer tokens,
not LLM tokenizer tokens**. The action-selection compact variant reduces LOC
17→14 while increasing lexical tokens 115→116, showing why one metric is not
enough. [Benchmark methodology and phase measurements](../bench/README.md) give
the complete scope. The Python comparison below remains a separate, honest
cross-language source-size study.

## Editor protocol bridge

An optional, standard-library-only Python bridge provides an LSP stdio server:

```sh
python tools/limits_lsp.py --lim-bin compiler/target/release/lim
```

Configure an editor's LSP client to launch that command with a trusted CLI path.
The bridge supports initialization, full document synchronization, published
diagnostics, document symbols, definition, references, completion and hover.
It converts source scalar columns to **0-based UTF-16** LSP positions, including
astral Unicode characters. Document URIs are opaque identifiers, never filesystem
paths to read. Open document text travels to `lim index - --json` on stdin. Editing
never invokes `run`, decrypts files, or executes the document.

The bridge caches the index by document version. Queries reuse that snapshot;
a full content change reindexes once, stale versions are ignored, and closing a
document removes it and clears diagnostics. It handles one request at a time and
uses a five-second backend deadline. It does not provide incremental parsing,
cross-file resolution, semantic types, rename edits, formatting or cancellation
of an active index request. In incomplete/invalid source, diagnostics remain
available but local queries are empty; completion falls back to keywords/builtins
instead of inventing bindings from a stale index.

Bounds: 16 MiB per transport message, 32 open documents, 16 MiB aggregate open
source, 8 MiB serialized index per document and 32 MiB aggregate serialized index
cache. Document URIs and string request IDs are capped at 4 KiB. Each query returns
at most 1,024 results; completion marks truncated results as incomplete. Definition,
reference and symbol arrays are capped, so narrow the query when working with very
large bindings. Python object overhead and subprocess allocations still require host RAM
supervision. A configured `lim` executable is trusted host configuration; source
text is passed as data and no shell is used. Backend stderr is not forwarded or
logged, so a failed index does not echo source or secret data into editor logs.

Tests can run without third-party Python packages:

```sh
LIM_BIN="$PWD/compiler/target/debug/lim" python -m unittest discover -s tools -p 'test_limits_lsp.py'
```

## Local workflows included here

| Example | Decision it implements | Verified result |
| --- | --- | --- |
| [task_router.lim](../examples/agents/task_router.lim) | Affordable action scoring with stable ties | `search 7` |
| [dependency_plan.lim](../examples/agents/dependency_plan.lim) | Stable dependency planning with a progress guard | research → draft → review → publish |
| [retry_policy.lim](../examples/agents/retry_policy.lim) | Retry classification and logical backoff ticks | 3 attempts, 3 ticks, success |
| [record_transform.lim](../examples/agents/record_transform.lim) | Schema checks and grouped aggregation | 48 cost units, done=2, queued=1 |
| [closure_budget.lim](../examples/agents/closure_budget.lim) | Independent closure-backed budget state | 0 and 2 remaining |
| [bounded_search.lim](../examples/agents/bounded_search.lim) | Bounded breadth-first route planning | start → issues → done |

`compiler/tests/agent_workflows.rs` statically checks every file in this directory,
executes both engines, compares stdout and errors, and verifies the expected
output. The fixtures contain their own assertions as well.

## Source size study

These are readable complete programs with the same fixtures, decision algorithms
and assertions, compared with straightforward Python equivalents reproduced below.
Python's usual list/dict print representation differs from Limits; the assertions
check equivalent decision values. Python equivalents use ordinary builtins such
as `all`, `set`, `dict.get` and `reversed` where those simplify the program.

Bytes count the UTF-8 files/code blocks exactly, including comments and final
newlines. Code LOC counts nonblank, non-comment lines, including braces and data
fixtures. No formatter, minifier, source compression or token estimator is used.
The samples are not a language benchmark or a representative workload corpus.
Model tokenization depends on the particular tokenizer, so bytes and LOC are not
LLM token counts. Runtime timing belongs to the separate pipeline benchmark.

| Workflow | Limits bytes | Limits code LOC | Python bytes | Python code LOC |
| --- | ---: | ---: | ---: | ---: |
| task_router.lim | 667 | 21 | 563 | 18 |
| dependency_plan.lim | 970 | 32 | 788 | 23 |
| retry_policy.lim | 858 | 23 | 686 | 19 |
| record_transform.lim | 989 | 23 | 802 | 19 |
| closure_budget.lim | 734 | 24 | 637 | 21 |
| bounded_search.lim | 1040 | 44 | 771 | 34 |

This sample does **not** demonstrate a LOC reduction over Python. Limits currently
pays for explicit braces, explicit map defaults and a deliberately smaller builtin
set. Its strengths in this comparison are explicit execution limits, predictable
scope/alias behavior and inspectable local decisions, not universal source brevity.
A future builtin should be justified by a repeated pattern and measured reduction
on a held-out workload, with effects, failure behavior and budget costs specified.

The comparison programs are included so the numbers can be independently checked.
To reproduce, use `len(source.encode("utf-8"))` for bytes and count lines where
`line.strip()` is nonempty and `not line.lstrip().startswith("#")` for code LOC.

### Python equivalent of task_router.lim

```python
budget = 8
actions = [
    {"name": "search", "cost": 3, "benefit": 9, "risk": 2},
    {"name": "browse", "cost": 10, "benefit": 15, "risk": 1},
    {"name": "summarize", "cost": 2, "benefit": 6, "risk": 0},
]
best = None
best_score = None
for action in actions:
    if action["cost"] > budget:
        continue
    score = action["benefit"] - action["risk"]
    if best is None or score > best_score:
        best = action
        best_score = score
assert best is not None, "no affordable action"
assert best["name"] == "search"
print(best["name"], best_score)
```

### Python equivalent of dependency_plan.lim

```python
def ready(dependencies, completed):
    return all(completed.get(name) is True for name in dependencies)

tasks = [
    {"id": "publish", "after": ["review"]},
    {"id": "draft", "after": ["research"]},
    {"id": "research", "after": []},
    {"id": "review", "after": ["draft"]},
]
completed = {}
plan = []
while len(plan) < len(tasks):
    progressed = False
    for task in tasks:
        id = task["id"]
        assert isinstance(id, str), "task id must be a string"
        if completed.get(id) is True or not ready(task["after"], completed):
            continue
        plan.append(id)
        completed[id] = True
        progressed = True
    assert progressed, "cycle, duplicate id, or unknown dependency"
assert plan == ["research", "draft", "review", "publish"]
print(plan)
```

### Python equivalent of retry_policy.lim

```python
def retryable(status):
    return status == 429 or 500 <= status < 600

responses = [429, 503, 200]
policy = {"max_attempts": 3, "delays": [1, 2, 4]}
state = {"attempts": 0, "backoff_ticks": 0, "done": False}
while state["attempts"] < policy["max_attempts"]:
    attempt = state["attempts"]
    assert attempt < len(responses), "missing simulated response"
    status = responses[attempt]
    state["attempts"] = attempt + 1
    if status == 200:
        state["done"] = True
        break
    if not retryable(status):
        break
    state["backoff_ticks"] += policy["delays"][attempt]
assert state["done"] and state["attempts"] == 3
assert state["backoff_ticks"] == 3
print(state)
```

### Python equivalent of record_transform.lim

```python
records = [
    {"id": "a", "status": "queued", "tokens": 12},
    {"id": "b", "status": "done", "tokens": 7},
    {"id": "c", "status": "done", "tokens": 5},
]
summary = {"by_status": {}, "cost_units": 0, "ids": []}
for record in records:
    assert isinstance(record["id"], str), "id must be a string"
    assert type(record["tokens"]) is int, "tokens must be an int"
    assert record["tokens"] >= 0, "tokens must be nonnegative"
    status = record["status"]
    assert isinstance(status, str), "status must be a string"
    groups = summary["by_status"]
    groups[status] = groups.get(status, 0) + 1
    summary["cost_units"] += record["tokens"] * 2
    summary["ids"].append(record["id"])
assert summary["by_status"] == {"done": 2, "queued": 1}
assert summary["cost_units"] == 48
print(summary)
```

### Python equivalent of closure_budget.lim

```python
def make_budget(initial):
    assert initial >= 0, "initial budget must be nonnegative"
    remaining = initial
    def take(cost):
        nonlocal remaining
        assert cost >= 0, "cost must be nonnegative"
        if cost > remaining:
            return False
        remaining -= cost
        return True
    def left():
        return remaining
    return {"take": take, "left": left}

budget = make_budget(10)
independent = make_budget(2)
assert budget["take"](3)
assert not budget["take"](8)
assert budget["take"](7)
assert budget["left"]() == 0
assert independent["left"]() == 2
print(budget["left"](), independent["left"]())
```

### Python equivalent of bounded_search.lim

```python
graph = {
    "start": ["docs", "issues"],
    "docs": ["reference"],
    "issues": ["done"],
    "reference": [],
    "done": [],
}
frontier = ["start"]
seen = {"start"}
parent = {}
head = 0
found = False
while head < len(frontier):
    assert head < 8, "expansion budget exhausted"
    node = frontier[head]
    head += 1
    if node == "done":
        found = True
        break
    for next in graph[node]:
        if next in seen:
            continue
        seen.add(next)
        parent[next] = node
        frontier.append(next)
assert found, "no route"
reverse = []
current = "done"
while current is not None:
    reverse.append(current)
    current = parent.get(current)
route = list(reversed(reverse))
assert route == ["start", "issues", "done"]
print(route)
```

