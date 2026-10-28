# Optional encrypted programs

Limits can encrypt UTF-8 `.lim` source using the maintained
[age](https://age-encryption.org/) executable. This adapter uses native age
X25519 recipient and identity keys and the standard authenticated age v1 binary
format. It does not implement a cipher or claim a special "military grade"
security level. Password mode, SSH recipients, age plugins, and armored input
are not supported by this adapter.

Install `age` and `age-keygen` from a trusted package manager or verified upstream
release. The Rust compiler/runtime has no new cryptography crate dependencies.
`age` is optional; ordinary unencrypted source works without it. Keep its
executable and the process `PATH` under host control.

## Keys and commands

Generate an identity in a private directory. The identity is a secret; the
recipient printed by `age-keygen` is public:

```sh
mkdir -m 700 agent-keys
age-keygen -o agent-keys/worker.agekey
age-keygen -y agent-keys/worker.agekey
```

Substitute the complete recipient printed above for `age1...`:

```sh
lim seal task.lim --recipient age1... --out task.lim.age --json
lim check task.lim.age --identity agent-keys/worker.agekey --json
lim index task.lim.age --identity agent-keys/worker.agekey --json
lim run task.lim.age --identity agent-keys/worker.agekey --engine vm --json
```

The encrypted source is authenticated and decrypted in memory before lexing,
checking, indexing, or execution. These operations do not create a temporary
plaintext file. `seal -` also accepts bounded UTF-8 plaintext from stdin.

Exporting plaintext requires an explicit destination:

```sh
lim unseal task.lim.age --identity agent-keys/worker.agekey --out restored.lim --json
```

`seal` and `unseal` refuse to overwrite existing destinations. Newly created
outputs are owner-only (`0600`) on Unix; use private directories and appropriate
ACLs on Windows. `unseal` deliberately creates a plaintext file requested by the
caller. Remove it according to the application's retention policy.

## Trust model

Encryption authorizes a key holder, not a type of intelligence. A human with the
same identity can decrypt exactly the same source as an AI agent. To restrict
access to an agent workload, let a trusted host choose and mount an identity
using service identity, a secret manager, or OS permissions. Do not include the
private key itself in prompts, source, source-control history, or diagnostic
logs. Passing an identity *path* is host configuration; it does not grant a `.lim`
program arbitrary filesystem access.

The identity file **and its parent directories** must be protected, host-owned,
and immutable for the operation. The adapter validates the canonical identity
as a bounded, regular native X25519 key file; `age` subsequently opens the same
canonical path. This is not a defense against a compromised host or another
process able to replace that protected path between validation and use. FIFOs,
directories and plugin identities are rejected. Key rotation should use a new
protected path and change the host's selected identity between invocations.

Plaintext exists in process memory and is available to the authorized compiler
and runtime. The adapter does not promise locked/zeroized memory, hide code from
the authorized runtime/operator, or prevent memory inspection by a privileged
host. Encryption does not supply OS isolation or network permissions; combine
it with the capability policy and supervised worker limits documented in
[production.md](production.md).

## Failure and work bounds

Source is capped at 16 MiB, ciphertext at 16 MiB plus 64 KiB, and the identity
file at 64 KiB. The age child has a ten-second deadline by default, concurrent
bounded input/output handling, no shell, and discarded stderr. On timeout or
failure the owned age child is killed/reaped, and both I/O workers are joined
before the operation returns. This relies on the trusted executable closing its
pipe descriptors when it exits: a descendant retaining inherited descriptors
can delay worker cleanup beyond the child deadline. The adapter does not kill a
process group; a host supervisor must own any descendant cleanup. Host file
access and executable startup also rely on a healthy trusted local filesystem
and OS. Do not configure an attacker-controlled executable.

Plaintext returned by a streaming decryptor is held privately until `age`
finishes successfully. Tampered/truncated ciphertext or a wrong identity returns
an error, never unauthenticated partial plaintext. Error messages do not include
key contents, decrypted source, child stderr, or identity paths. Index and run
results intentionally expose authorized code intelligence and program output.

## Verification

Default tests cover missing tools, recipient-option injection, malformed and
plugin identities, non-regular identities, input caps, process deadlines, bounded
output and sanitized failures. Tests that require the real external tools are
explicitly marked ignored instead of pretending to perform cryptography when
`age` is absent. Enable them with:

```sh
cargo test --manifest-path compiler/Cargo.toml -- --ignored
```

CI configures a separate Linux job with real `age` and `age-keygen` to run all
ignored authenticated-encryption integration tests, including maximum-size source
round trips, final-chunk authentication and wrong-key rejection across CLI commands.

When the binaries are not on `PATH`, set `LIMITS_AGE_BINARY` and
`LIMITS_AGE_KEYGEN` to their trusted paths for these integration tests. They verify
randomized encryption, Unicode and empty-source round trips, wrong-key and
ciphertext-tampering rejection, direct encrypted run/check/index, explicit
plaintext export, owner-only Unix output modes, and refusal to overwrite files.
