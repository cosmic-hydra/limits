# Agent platform contract

Limits provides a small deterministic computation language, a stack-safe VM,
explicit execution budgets, inspectable code, and a versioned JSON CLI. These are
concrete interfaces for agents and editors, not a claim that any model will
produce correct programs or that this language dominates every other language.

## Discover, generate, check, execute

```sh
lim capabilities
lim check examples/agents/task_router.lim --json
lim run examples/agents/task_router.lim --engine vm --json --sandbox restricted
```

Pass `-` instead of a file to supply bounded UTF-8 source through stdin. Ordinary
human `run` output remains unchanged. `--json` returns exactly one JSON document
on stdout, with program stdout inside the `stdout` string. Program output cannot
inject envelope fields. Do not parse printed Limits maps as JSON.

Responses have `schema_version: 1`, `command`, `ok`, `source`, `diagnostics`,
`diagnostic_count`, and `diagnostics_truncated`. Run responses also have `engine`
and `stdout`; `engine` is null for host/usage failures before execution starts.
Query responses store results in `data`. Exit codes: 0 success, 1 program/static
error, 2 host or usage error. Index queries may return useful `data` alongside
static errors and exit 1. A lexer/parser error returns `data: null`.

Diagnostics include kind, stable code, severity, message, 1-based Unicode-scalar
line/column, and an advisory hint. Host/usage failures have null positions. Codes
are derived from trusted error kinds, never user assertion text. JSON diagnostics
are capped at 1,024 entries and 16 KiB per message, with explicit counts and
truncation metadata. The internal library retains complete errors. Hints are not
executable fixes, permission grants, or reliable security classifications.

JSON run defaults to `restricted` policy: 1 MiB aggregate program stdout and a
five-second cooperative execution deadline. Explicit `--sandbox unrestricted`
removes that deadline, but JSON capture still defaults to 1 MiB unless the host
sets `--max-output-bytes`. These are host choices, never source-level grants.

## Semantic indexing and code intelligence

```sh
lim index example.lim
lim symbols example.lim --limit 50
lim definition example.lim --line 4 --col 12
lim references example.lim --line 1 --col 5 --limit 100
lim complete example.lim --line 4 --col 12 --prefix act --limit 20
```

`index` returns the full deterministic symbol/scope/reference graph. Other
queries default to 100 results and accept `--limit 1..10000`; their result includes
`truncated`. Positions are 1-based Unicode scalar columns; ranges are end
exclusive. Builtins have no source declaration range. Identifiers in comments,
strings and map keys are not falsely treated as references.

The index follows lexical scopes, shadowing, function parameters and loop
variables. Definitions expose **lexical candidates**, including late-declaration
fallbacks. They do not pretend to know which dynamic branch or closure invocation
has executed. Reference queries conservatively include references for which the
symbol is a candidate. Captures are conservative direct captures. Repeated
bindings preserve all declaration locations; IDs are deterministic for the same
source, not persistent identities across edits.

The optional Python 3.12 editor bridge uses only the standard library:

```sh
python tools/limits_lsp.py --lim-bin /absolute/path/to/lim
```

It supports full-document synchronization, diagnostics, document symbols,
definition, references, completion and hover. It translates LSP UTF-16 positions
to the language's scalar columns, caches by document version and uses stdin
rather than temporary source files. It invokes indexing only: editing never
executes a Limits program. Incomplete syntax produces parse diagnostics; this is
not an incremental recovery parser or a full IDE refactoring engine. See
[agents.md](agents.md) for the bridge's framing and resource limits.

## Capability sandbox

```sh
lim run example.lim --engine vm --sandbox restricted --max-steps 10000 --max-depth 32
lim run example.lim --engine vm --sandbox pure --allow-builtin len --allow-builtin get
lim run example.lim --deny-builtin print --max-output-bytes 1024 --timeout-ms 100
```

Profiles: `unrestricted` preserves legacy builtin permissions; `restricted`
allows existing builtins but bounds output/deadline; `pure` additionally denies
print/push/pop. The latter is a builtin-capability profile, not a proof of
functional purity: local assignments and captured state remain language features.
Repeated `--allow-builtin` establishes an explicit allowlist. `--deny-builtin`
subtracts permissions afterwards, independently of flag order.

Permission checks attach to actual native builtin identities. Aliasing a builtin,
storing it in a map/list, passing it as a parameter or closing over it cannot
bypass the policy. A user function named `print` does not gain native I/O rights.
Compiled programs do not contain authority; the same program can be executed
under different policies. The host library can provide a shared
`CancellationToken` for cooperative cancellation from a supervisor.

The stdout budget counts UTF-8 bytes including separators/newlines. It is not a
cap on diagnostic/control-plane output. Deadline and cancellation checks occur at
statement, call, loop, and output boundaries; they cannot interrupt a blocking
host writer, source stdin read, or a single native operation. No filesystem,
network, process, environment or secret-reading builtin is introduced. Use an
OS-supervised worker with memory/time/I/O limits for untrusted programs; this
language layer is not an OS sandbox. See [production.md](production.md).

## Compact data helpers

- `get(map, string_key, default?)`: missing keys use the default (or nil), while
  existing nil values remain nil. Arguments, including defaults, are eager.
- `has(map, string_key)`: key membership, distinct from non-nil value checks.
- `join(list_of_strings, string_separator)`: strict string joining without mutation.
- `sum(list_of_numbers)`: checked integer addition with normal float promotion.

`join`/`sum` accept at most one million elements. Joined strings are capped at
16 MiB before allocation. Empty joins return `""`; empty sums return integer 0.
All use ordinary call accounting and capability checks, and preserve aliases.

The matched report in `bench/agent_compact` falls from 28 to 8 non-comment source
lines and 143 to 83 Limits lexer tokens. Those are language tokens, not model
vocabulary tokens. Six readable workflow examples are still longer than the
included Python equivalents. Source brevity is a measured workload property,
not a universal slogan; see [bench/README.md](../bench/README.md).

## Encrypted source

Use optional authenticated `age` encryption with a protected agent workload key:

```sh
lim seal policy.lim --recipient age1... --out policy.lim.age --json
lim run policy.lim.age --identity /protected/agent.key --engine vm --json
lim index policy.lim.age --identity /protected/agent.key
lim unseal policy.lim.age --identity /protected/agent.key --out recovered.lim
```

Install a trusted `age` executable separately. Outputs are new files only; no
existing file is overwritten, and Unix output files are created with mode 0600.
Direct encrypted execution/indexing decrypts in memory, with no plaintext source
temporary file. Explicit `unseal` intentionally writes plaintext.

Encryption authorizes whoever holds the matching key. It cannot distinguish an
AI agent from a human with that key. Workload identities, key brokers, secret
provisioning and host access policy must protect agent keys. We do not implement
custom cryptography, claim "military grade", or claim source is intelligible only
to AI. Read [encryption.md](encryption.md) before deployment, including immutable
identity-path, trusted executable and process-supervision requirements.

## Measured compilation

The code generator now uses deterministic hash-indexed slots and shared scope
layouts. For 8,000 declarations / 16,001 source lines, measured release codegen
median dropped from 392.662 ms to 5.824 ms in the recorded run (~67.4x). This is a
specific compiler microbenchmark, not a comparison against other languages or a
claim of equivalent improvement in lexing, checking, execution or every program.
Raw measurements and a dependency-free pipeline benchmark are in `bench/`.
