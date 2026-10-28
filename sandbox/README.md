# limits-sandbox

An OS-level sandbox launcher for compiled Limits programs. It is a **backstop**:
the Limits compiler and runtime already enforce a program's declared
capabilities (`uses fs.read;`, `--allow fs.read=DIR`, ...). This launcher
makes the Linux kernel enforce them as well, so a bug in the compiler, the
generated C or the runtime cannot turn into access the program never had.

Zero dependencies (raw syscalls through a small `extern "C"` layer), Linux
x86_64 only.

```
limits-sandbox [--read DIR]... [--write DIR]... [--net none] [--cpu-sec N]
               [--mem-mb N] [--fsize-mb N] [--nproc N] [--wall-sec N]
               [--env NAME[=VALUE]]... [--allow-degraded] [--quiet] -- PROGRAM ARGS...
```

Defaults: `--cpu-sec 10 --mem-mb 512 --fsize-mb 64 --nproc 1 --wall-sec 30`,
no readable or writable directories besides the system libraries.

On exit it prints one line to stderr and exits with the matching code:

| line | exit code |
|---|---|
| `limits-sandbox: exit N` | N |
| `limits-sandbox: killed by signal S (reason)` | 128+S |
| `limits-sandbox: timeout (wall limit Ns)` | 137 |
| setup errors (`limits-sandbox: ...`) | 125 |
| child setup failed before exec | 127 |

With `--quiet` the `exit N` line is not printed, and a child killed by a
signal makes the launcher die of the same signal (no line either), so a
caller sees the program's own exit status. `limits run`/`limits test` use it.

The program never outlives the launcher: the child sets `PR_SET_PDEATHSIG`
(SIGKILL) before exec, so if the launcher is killed (for example by a
caller's own timeout) the kernel kills the program too, instead of leaving it
running with no wall limit.

## Threat model

The program being run is untrusted native code: assume a compiler or runtime
bug lets it run arbitrary instructions in its own process. The sandbox
must keep it to the declared files, no network, no new processes, and bounded
CPU, memory, disk and wall time. The launcher itself, the kernel and the
files under the granted directories are trusted.

## What is enforced

Applied in the forked child, in this order, right before `execve`:

1. **rlimits**: `RLIMIT_CPU` (soft = N s, sends SIGXCPU; hard = N+1 s,
   SIGKILL), `RLIMIT_AS` (`--mem-mb`, so `malloc` fails), `RLIMIT_FSIZE`
   (SIGXFSZ), `RLIMIT_NPROC`, `RLIMIT_NOFILE` = 64, `RLIMIT_CORE` = 0.
2. **`PR_SET_NO_NEW_PRIVS`**: setuid binaries and file capabilities stop working.
3. **Landlock** (ABI found with `landlock_create_ruleset(NULL, 0, VERSION)`).
   Every filesystem right the kernel knows is handled, so anything not granted
   is denied:
   - read (file + directory listing): `/lib`, `/lib64`, `/usr/lib`,
     `/usr/lib64`, `/etc/ld.so.cache`, and each `--read DIR`;
   - read + execute: the program itself and its ELF interpreter (`PT_INTERP`,
     the dynamic loader). Nothing else can be executed;
   - everything except execute (create, write, truncate, delete, rename...):
     each `--write DIR`;
   - ABI >= 4: TCP bind and connect denied. ABI >= 6: abstract unix socket
     and signal scoping (no signals to processes outside the sandbox).
4. **seccomp-BPF**: checks the architecture (x86_64, x32 numbers rejected),
   then allows 72 syscalls plus 4 with argument checks (file I/O, memory, signals for itself,
   time, futex, `getrandom`, ...). Argument checks: `ioctl` only with
   `TCGETS`; `kill`/`tgkill` only on its own pid; `execve` only with the
   exact path pointer the launcher passes. Anything else (`socket`,
   `connect`, `fork`, `clone`, `clone3`, `execveat`, `ptrace`, `mount`,
   `bpf`, ...) gets `SECCOMP_RET_KILL_PROCESS`, reported as signal 31.

The parent enforces `--wall-sec` by sending SIGKILL.

If Landlock or seccomp is missing in the kernel or container, the launcher
prints a warning for each missing layer and refuses to run. Pass
`--allow-degraded` to run without them (the warnings are still printed).

## What is NOT enforced

- **Side channels and timing**: cache and Spectre-style attacks, and
  measuring timing, are not prevented. CPU time and wall time are bounded but
  not hidden.
- **Kernel bugs**: a kernel exploit through an allowed syscall gets past
  every layer. Keeping the syscall list short reduces this risk but cannot
  remove it.
- **Data the program may read**: anything under `--read`/`--write` and the
  system library directories can be read, and the program's stdout/stderr can
  carry it out.
- **Environment variables you choose to pass**: by default the child gets only
  `PATH`, `LANG`, `LANGUAGE`, `LC_*`, `TERM` and `TZ` from the launcher; every
  other variable (API tokens, cloud credentials, `HOME`, ...) is dropped.
  `--env NAME` copies one more variable from the launcher and
  `--env NAME=VALUE` sets one; anything passed this way is readable by the
  program. (Earlier versions passed the whole environment through.)
- **Already-open file descriptors**: inherited fds (stdin, stdout, stderr,
  anything else the caller leaves open) are not checked by Landlock.
- **Second `execve`**: the pointer check means another exec needs the same
  path string at the same address. Landlock still only lets it run the same
  program or the loader, so the program could re-run itself but nothing else.
- **Disk and memory pressure**: `RLIMIT_FSIZE` limits each file, not the
  total, so many files up to the limit can still be written in `--write` dirs.
  `RLIMIT_AS` limits address space, not RSS.
- **`RLIMIT_NPROC`** counts per user and does nothing for root. Process
  creation is blocked by seccomp, not by this limit.
- **Non-x86_64 / non-Linux**: not supported (compile error).
- **Older kernels**: with Landlock ABI < 4 the network is blocked only by
  seccomp (`socket` kills). With ABI < 3, `truncate` on files outside the
  write set is not blocked. With ABI < 6, signals to same-uid processes
  are blocked only by the `kill`/`tgkill` pid check (other signal syscalls
  such as `rt_sigqueueinfo` are not in the allowlist).

## Tests

`cargo test` compiles a C probe with `cc` and checks that: an allowed read
works; `/etc/passwd` outside `--read` gives EACCES/EPERM; a write outside
`--write` is denied; `socket`, `fork` and `execve("/bin/sh")` kill the
process; allocating more than `--mem-mb` fails; a CPU loop is killed by
`--cpu-sec`; the wall timeout works; the exit code is passed through. It
also builds a hello program and a file-reading program with
`../native/target/release/limits` and runs them in the sandbox. That test is
skipped, with a message, if the binary is missing.
