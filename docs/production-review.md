# 25-specialist production review

This review starts from commit `5afd018c5425507edd8bf0f46552dcbe301fd7ec`
(the agent-tooling PR). Twenty-five separately assigned engineering reviewers
worked in bounded waves, with independent cross-review and one integration owner.
The workspace permits six active subagents alongside the integrator; this was not
25 simultaneous workers. Clean findings were retained rather than converted
into speculative production changes.

## Verified defects and corrections

- **Short-circuit compilation:** branch fusion mistook an ending subexpression
  for the whole condition. False `and` conditions could take the true branch,
  leave operands behind, or make loops run to their step limit. Fusion now requires
  a complete one-instruction condition. Exact-output/error differential coverage
  includes 375 composed programs and every step cutoff through termination.
- **Rendering memory guards:** oversized scalar strings and escaped map keys
  could allocate beyond the rendering limit before failing. Incremental bounded
  formatting now checks before reserve; deferred map keys share a queue budget.
  Allocation-observing regressions failed on the baseline implementation.
- **Print framing:** multi-argument accounting omitted one byte for the final
  newline. Exact-limit output succeeds; one byte over is rejected before writing.
- **Frontend contracts:** malformed public token streams no longer hide tokens
  after an early EOF, missing identifiers point at the real EOF, and the public
  resolver validates caller-built ASTs before recursive traversal.
- **Line endings:** bare CR previously failed to terminate comments or advance
  source lines. LF, CRLF and CR now agree across lexer, semantic index and editor
  positions, without changing the value of raw string contents.
- **CLI failure handling:** invalid-Unicode arguments, broken help output and
  failed stderr diagnostics no longer panic into exit 101. They retain documented
  host/usage/program statuses and structured errors where output is available.
- **Encryption lifecycle:** owned age pipe workers are joined after completion
  or direct-child cleanup, with timeout/backpressure regressions. Authenticated
  partial plaintext remains private until the entire age operation succeeds.
- **LSP robustness:** framing, short reads, duplicate fields, numeric/depth guards,
  schema validation, backend pipe-drain deadlines, null host diagnostics, ranked
  definitions, Unicode completion and shutdown state were checked and corrected.
  Coordinate caching and result caps prevent repeated full-document scans before
  truncation. Oversized outgoing responses are handled separately from malformed
  incoming transport; valid Unicode is encoded directly in UTF-8.
- **Discovery:** the capability catalog now advertises host and usage diagnostic
  codes as well as the four language-error categories.

## Reviewer assignments and retained evidence

| SWE | Specialization | Result and evidence |
| --- | --- | --- |
| 01 | VM frames, operand and optimized slot invariants | Clean audit; existing suites and additional review-time exact engine comparisons. No speculative VM change. |
| 02 | Tree continuations and control flow | Clean audit; `compiler/tests/swe02_tree.rs` covers suspended roots, return truncation and error recovery. |
| 03 | GC roots, cycles and destruction | Clean audit; `swe03_gc.rs` covers pending roots, iterator snapshots, retained globals and subprocess-isolated deep cleanup. |
| 04 | Lexer/parser/public token input | Fixed hidden token suffixes and EOF locations; `swe04_frontend.rs`. |
| 05 | Shared values and standard operations | Fixed rendering allocations; `swe05_values.rs` includes before/after allocation evidence and exact byte boundaries. |
| 06 | Encryption subprocess lifecycle | Fixed detached workers; helper backpressure tests and `swe06_crypto.rs` with real late-authentication rejection. |
| 07 | CLI input, flags and sinks | Fixed argv/help/stderr panics; `swe07_cli.rs`. |
| 08 | JSON protocol and diagnostics | Clean audit; all Unicode scalars, trusted codes, escaping and existing protocol suites checked. No narrower total-byte cap was falsely inferred from per-message/count caps. |
| 09 | LSP framing and backend lifecycle | Fixed transport/backend/query issues; `tools/test_swe09_transport.py`. |
| 10 | Editor synchronization and Unicode | Found CR, ordering, combining-mark and conversion-amplification issues; `tools/test_swe10_documents.py`. |
| 11 | Semantic bindings and public resolver | Added AST validation before resolution; `swe11_semantic.rs` covers deep ASTs, captures, scopes and roles. |
| 12 | Source coordinates and query boundaries | Independent source oracle, all newline forms, astral names and exact ranges; `swe12_positions.rs`. |
| 13 | Capabilities, deadlines and cancellation | Clean authority audit; `swe13_sandbox.rs` and deterministic deadline reuse unit test. |
| 14 | Code generation and branch optimizations | Found/fixed short-circuit fusion; `swe14_codegen.rs`. |
| 15 | Independent generated differential testing | `swe15_differential.rs`: 375 composed sources, exact stdout/errors, charged-step prefixes and depth caps. |
| 16 | Linux/Windows/macOS portability | Audited cfg boundaries; Unicode/spaced native-path and line-ending tests in `swe16_portability.rs`; actual OS validation is CI. |
| 17 | Filesystem output safety | Clean audit; collision aliases, permissions, partial-write cleanup, caps and explicit real maximum-size age test in `swe17_filesystem.rs`. |
| 18 | Independent authentication boundary | Clean audit; malformed envelopes, sanitized failures and real wrong-key/checksum tests in `swe18_authentication.rs`. |
| 19 | Resource exhaustion and bounded work | Found print framing bypass; `swe19_resources.rs` checks rejection before writing and retained prior output. |
| 20 | Rust embedding and lifecycle | Clean audit; malformed-input isolation, exported closures, compiled clone freshness and supervisor cancellation in `swe20_embedding.rs`. |
| 21 | Compilation performance acceptance | Source scaling/determinism inspected; `swe21_compile.rs` and `bench/swe21_compile.jsonl`. |
| 22 | VM throughput and allocation acceptance | Paired baseline/current timings and allocation probes recorded in `bench/swe22_runtime.jsonl`; no core change. |
| 23 | Agent workflows and source ergonomics | Found catalog-code mismatch; `swe23_agent_workflows.rs` tests discover/check/repair/run and validates source-size claims. |
| 24 | Documentation and contract conformance | Verified and corrected SPEC/production/agent/encryption documentation against real behavior. |
| 25 | Independent release acceptance | Rechecked scope, CI discovery and trust contracts; caught the large-Unicode LSP response availability bug before release acceptance. |

Test filenames without a directory in this table are under `compiler/tests/`.
Tests intentionally optional without real age tools are marked ignored rather
than reported as cryptographic successes. The authenticated-encryption CI job
runs **all** ignored real-tool tests with age and age-keygen installed.

## Performance interpretation

Current five-sample release code-generation medians at scope widths
1,000/2,000/4,000/8,000 were 0.566/1.150/2.840/5.243 ms. The historical 8,000-width
392.662 ms result used the same source shape; it is not a contemporaneous control
or a universal speed guarantee. Slot allocation remains deterministic and shared
layouts preserve late-binding semantics.

The VM reviewer built the baseline separately and compared opposite run orders.
Baseline fib25 VM medians were 32.43 and 40.78 ms; current medians were 37.92 and
41.26 ms. Tree medians also varied substantially. The adjacent reverse-order VM
pair differed by 1.18%, but this does not establish a precise regression bound.
No gross regression was demonstrated, and historical snapshots alone cannot
attribute timing differences to these fixes. Raw data and context are retained.

A review-time allocation probe found constant allocation counts for frame-only
fib18/fib25 (30 allocations, eight reallocations). A captured-function variant
allocates more often while its peak live payload stayed approximately flat for
this workload. These are workload measurements, not a complete reclamation proof.

## Acceptance and remaining host responsibilities

The release gate is exact behavior/regression testing in debug and release,
all-target Clippy with warnings denied, formatting, real authenticated-encryption
checks, independent JSON/editor tests, and CI on Linux, Windows and macOS. The PR
records final counts and CI links for the integrated commit. Timing numbers are
not flaky pass/fail thresholds.

Execution steps, native-operation work guards and cooperative cancellation are
not a hard RAM or wall-clock sandbox. Trusted executable/key-path requirements,
process descendants, blocking host I/O and OS memory/time supervision remain
explicit deployment responsibilities. No AI-only encryption, universal language
superiority, formal verification or cryptographic certification is claimed.
