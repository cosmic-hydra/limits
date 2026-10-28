//! limits-sandbox: an OS-level backstop for compiled Limits programs.
//!
//! The Limits compiler and runtime already refuse undeclared capabilities;
//! this launcher makes the kernel refuse them too, so a compiler or runtime
//! bug cannot turn into real file, network or process access. Layers, all
//! applied in the forked child right before `execve`:
//!   1. setrlimit (CPU, address space, file size, processes, fds, core=0)
//!   2. PR_SET_NO_NEW_PRIVS
//!   3. Landlock (filesystem allowlist; TCP bind/connect denied on ABI >= 4)
//!   4. seccomp-BPF syscall allowlist (anything else kills the process)
//!
//! The parent enforces the wall-clock limit and reports the outcome.

use std::ffi::{CString, OsStr};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
compile_error!("limits-sandbox currently supports Linux x86_64 only");

// ---------------------------------------------------------------- libc layer

#[repr(C)]
struct RLimit {
    cur: u64,
    max: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

#[repr(C)]
struct RulesetAttr {
    fs: u64,
    net: u64,
    scoped: u64,
}

#[repr(C, packed)]
struct PathBeneath {
    allowed: u64,
    parent_fd: i32,
}

extern "C" {
    fn syscall(n: i64, ...) -> i64;
    fn fork() -> i32;
    fn execve(p: *const i8, a: *const *const i8, e: *const *const i8) -> i32;
    fn waitpid(pid: i32, status: *mut i32, opts: i32) -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
    fn getpid() -> i32;
    fn getppid() -> i32;
    fn setrlimit(res: i32, lim: *const RLimit) -> i32;
    fn prctl(op: i32, ...) -> i32;
    fn open(p: *const i8, flags: i32, ...) -> i32;
    fn close(fd: i32) -> i32;
    fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    fn _exit(code: i32) -> !;
    fn signal(sig: i32, handler: usize) -> usize;
    fn raise(sig: i32) -> i32;
    fn __errno_location() -> *mut i32;
}

const SYS_LL_CREATE: i64 = 444;
const SYS_LL_ADD_RULE: i64 = 445;
const SYS_LL_RESTRICT: i64 = 446;
const SYS_SECCOMP: i64 = 317;
const O_PATH: i32 = 0o10000000;
const O_CLOEXEC: i32 = 0o2000000;
const PR_SET_NO_NEW_PRIVS: i32 = 38;
const PR_GET_SECCOMP: i32 = 21;
const PR_SET_PDEATHSIG: i32 = 1;
const WNOHANG: i32 = 1;
const SIGKILL: i32 = 9;

fn errno() -> i32 {
    // SAFETY: __errno_location always returns a valid thread-local pointer.
    unsafe { *__errno_location() }
}

// ------------------------------------------------------------------ options

#[derive(Default)]
struct Opts {
    read: Vec<PathBuf>,
    write: Vec<PathBuf>,
    cpu_sec: u64,
    mem_mb: u64,
    fsize_mb: u64,
    nproc: u64,
    wall_sec: u64,
    allow_degraded: bool,
    /// `--quiet`: no line for a normal exit; a death by signal is passed on
    /// by dying of the same signal (so callers see it as the program's).
    quiet: bool,
    env: Vec<String>,
    prog: Vec<String>,
}

const USAGE: &str = "usage: limits-sandbox [--read DIR]... [--write DIR]... [--net none] [--cpu-sec N] [--mem-mb N] [--fsize-mb N] [--nproc N] [--wall-sec N] [--env NAME[=VALUE]]... [--allow-degraded] [--quiet] -- PROGRAM ARGS...";

fn die(msg: &str) -> ! {
    eprintln!("limits-sandbox: {msg}");
    std::process::exit(125)
}

fn parse(args: Vec<String>) -> Opts {
    let mut o = Opts {
        cpu_sec: 10,
        mem_mb: 512,
        fsize_mb: 64,
        nproc: 1,
        wall_sec: 30,
        ..Opts::default()
    };
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--" {
            o.prog = it.collect();
            break;
        }
        if a == "--allow-degraded" {
            o.allow_degraded = true;
            continue;
        }
        if a == "--quiet" {
            o.quiet = true;
            continue;
        }
        let v = it
            .next()
            .unwrap_or_else(|| die(&format!("{a} needs a value\n{USAGE}")));
        let num = || {
            v.parse::<u64>()
                .unwrap_or_else(|_| die(&format!("{a}: bad number '{v}'")))
        };
        match a.as_str() {
            "--read" => o.read.push(canon(&v)),
            "--write" => o.write.push(canon(&v)),
            "--net" if v == "none" => {}
            "--net" => die("--net: only 'none' is supported"),
            "--cpu-sec" => o.cpu_sec = num(),
            "--mem-mb" => o.mem_mb = num(),
            "--fsize-mb" => o.fsize_mb = num(),
            "--nproc" => o.nproc = num(),
            "--wall-sec" => o.wall_sec = num(),
            "--env" if !v.is_empty() && !v.starts_with('=') => o.env.push(v),
            "--env" => die("--env: expected NAME or NAME=VALUE"),
            _ => die(&format!("unknown option '{a}'\n{USAGE}")),
        }
    }
    if o.prog.is_empty() {
        die(USAGE);
    }
    o
}

/// Variables passed to the child by default: none of them normally hold
/// secrets. Everything else (tokens, keys, cloud credentials) is dropped
/// unless named with `--env`.
const ENV_ALLOW: &[&str] = &["PATH", "LANG", "LANGUAGE", "TERM", "TZ"];

fn env_allowed(k: &[u8]) -> bool {
    ENV_ALLOW.iter().any(|a| a.as_bytes() == k) || k.starts_with(b"LC_")
}

/// Build the child's environment: the allowlist, then `--env` entries
/// (`NAME` copies the launcher's value, `NAME=VALUE` sets it). Later wins.
fn child_env(extra: &[String]) -> Vec<Vec<u8>> {
    let mut out: Vec<(Vec<u8>, Vec<u8>)> = std::env::vars_os()
        .filter(|(k, _)| env_allowed(k.as_bytes()))
        .map(|(k, v)| (k.into_vec(), v.into_vec()))
        .collect();
    for e in extra {
        let (k, v) = match e.split_once('=') {
            Some((k, v)) => (k.as_bytes().to_vec(), Some(v.as_bytes().to_vec())),
            None => (
                e.as_bytes().to_vec(),
                std::env::var_os(e).map(|v| v.into_vec()),
            ),
        };
        out.retain(|(ok, _)| *ok != k);
        if let Some(v) = v {
            out.push((k, v));
        }
    }
    out.into_iter()
        .map(|(k, v)| [k, b"=".to_vec(), v].concat())
        .collect()
}

fn canon(p: &str) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|e| die(&format!("{p}: {e}")))
}

fn cstr(b: &[u8]) -> CString {
    CString::new(b).unwrap_or_else(|_| die("argument contains a NUL byte"))
}

/// Resolve PROGRAM like execvp would, to an absolute canonical path.
fn resolve(prog: &str) -> PathBuf {
    if prog.contains('/') {
        return canon(prog);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|d| d.join(prog))
        .find(|p| p.is_file())
        .map(|p| canon(&p.to_string_lossy()))
        .unwrap_or_else(|| die(&format!("{prog}: not found in PATH")))
}

/// The ELF PT_INTERP (dynamic loader) of `exe`, if any.
fn interpreter(exe: &Path) -> Option<PathBuf> {
    let d = std::fs::read(exe).ok()?;
    let u16_at = |o: usize| Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?) as usize);
    let u64_at = |o: usize| Some(u64::from_le_bytes(d.get(o..o + 8)?.try_into().ok()?) as usize);
    if d.get(0..5)? != b"\x7fELF\x02" {
        return None;
    }
    let (phoff, phsz, phnum) = (u64_at(0x20)?, u16_at(0x36)?, u16_at(0x38)?);
    for i in 0..phnum {
        let ph = phoff + i * phsz;
        if d.get(ph..ph + 4)? == [3, 0, 0, 0] {
            let (off, sz) = (u64_at(ph + 8)?, u64_at(ph + 32)?);
            let s = d.get(off..off + sz)?.split(|&b| b == 0).next()?;
            return std::fs::canonicalize(OsStr::from_bytes(s)).ok();
        }
    }
    None
}

// ----------------------------------------------------------------- landlock

const LL_EXECUTE: u64 = 1;
const LL_WRITE_FILE: u64 = 2;
const LL_READ_FILE: u64 = 4;
const LL_READ_DIR: u64 = 8;
const LL_TRUNCATE: u64 = 1 << 14;
const LL_IOCTL_DEV: u64 = 1 << 15;
const LL_FILE_RIGHTS: u64 = LL_EXECUTE | LL_WRITE_FILE | LL_READ_FILE | LL_TRUNCATE | LL_IOCTL_DEV;

fn landlock_abi() -> i64 {
    // SAFETY: landlock_create_ruleset(NULL, 0, VERSION) only queries the ABI.
    unsafe { syscall(SYS_LL_CREATE, std::ptr::null::<u8>(), 0usize, 1u32) }
}

/// Build a ruleset fd in the parent; the child only calls restrict_self.
fn landlock_ruleset(abi: i64, o: &Opts, exe: &Path) -> Result<i32, String> {
    let mut fs = (1u64 << 13) - 1; // ABI 1: EXECUTE .. MAKE_SYM
    if abi >= 2 {
        fs |= 1 << 13; // REFER
    }
    if abi >= 3 {
        fs |= LL_TRUNCATE;
    }
    if abi >= 5 {
        fs |= LL_IOCTL_DEV;
    }
    let attr = RulesetAttr {
        fs,
        net: if abi >= 4 { 3 } else { 0 }, // BIND_TCP | CONNECT_TCP, no rules => denied
        scoped: if abi >= 6 { 3 } else { 0 }, // abstract unix sockets | signals
    };
    let size = match abi {
        1..=3 => 8usize,
        4 | 5 => 16,
        _ => 24,
    };
    // SAFETY: attr is a valid ruleset_attr and `size` never exceeds its length.
    let fd = unsafe { syscall(SYS_LL_CREATE, &attr as *const RulesetAttr, size, 0u32) } as i32;
    if fd < 0 {
        return Err(format!(
            "landlock_create_ruleset failed (errno {})",
            errno()
        ));
    }
    let mut rules: Vec<(PathBuf, u64)> = Vec::new();
    for lib in [
        "/lib",
        "/lib64",
        "/usr/lib",
        "/usr/lib64",
        "/etc/ld.so.cache",
    ] {
        rules.push((PathBuf::from(lib), LL_READ_FILE | LL_READ_DIR));
    }
    rules.push((exe.to_path_buf(), LL_READ_FILE | LL_EXECUTE));
    if let Some(interp) = interpreter(exe) {
        rules.push((interp, LL_READ_FILE | LL_EXECUTE));
    }
    rules.extend(
        o.read
            .iter()
            .map(|d| (d.clone(), LL_READ_FILE | LL_READ_DIR)),
    );
    rules.extend(o.write.iter().map(|d| (d.clone(), fs & !LL_EXECUTE)));
    for (path, want) in rules {
        let c = cstr(path.as_os_str().as_bytes());
        // SAFETY: c is a valid NUL-terminated path; O_PATH opens no data.
        let pfd = unsafe { open(c.as_ptr(), O_PATH | O_CLOEXEC) };
        if pfd < 0 {
            continue; // e.g. /lib64 absent on this distro
        }
        let allowed = if path.is_dir() {
            want & fs
        } else {
            want & fs & LL_FILE_RIGHTS
        };
        let rule = PathBeneath {
            allowed,
            parent_fd: pfd,
        };
        // SAFETY: rule is a packed landlock_path_beneath_attr, rule type 1.
        let r = unsafe { syscall(SYS_LL_ADD_RULE, fd, 1u32, &rule as *const PathBeneath, 0u32) };
        let e = errno();
        // SAFETY: pfd was opened above and is closed exactly once.
        unsafe { close(pfd) };
        if r < 0 {
            return Err(format!(
                "landlock_add_rule({}) failed (errno {e})",
                path.display()
            ));
        }
    }
    Ok(fd)
}

// ------------------------------------------------------------------ seccomp

const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
const RET_ALLOW: u32 = 0x7fff_0000;
const RET_KILL_PROCESS: u32 = 0x8000_0000;
const LD_ABS: u16 = 0x20;
const JEQ: u16 = 0x15;
const JGE: u16 = 0x35;
const RET: u16 = 0x06;
const TCGETS: u32 = 0x5401;

/// x86_64 syscalls a compiled Limits program (C runtime + glibc) needs.
#[rustfmt::skip]
const ALLOWED: &[u32] = &[
    0, 1, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, // read write close stat fstat lstat lseek mmap mprotect munmap brk sigaction sigprocmask sigreturn
    17, 18, 19, 20, 21, 24, 25, 28, 32, 33, 35, 39, // pread pwrite readv writev access sched_yield mremap madvise dup dup2 nanosleep getpid
    60, 63, 72, 74, 75, 77, 79, 82, 83, 84, 87, 89, // exit uname fcntl fsync fdatasync ftruncate getcwd rename mkdir rmdir unlink readlink
    96, 97, 102, 104, 107, 108, 110, 131, 158, 186, // gettimeofday getrlimit getuid getgid geteuid getegid getppid sigaltstack arch_prctl gettid
    201, 202, 204, 217, 218, 221, 228, 229, 230, 231, // time futex sched_getaffinity getdents64 set_tid_address fadvise64 clock_gettime clock_getres clock_nanosleep exit_group
    257, 258, 262, 263, 264, 267, 269, 273, 302, // openat mkdirat newfstatat unlinkat renameat readlinkat faccessat set_robust_list prlimit64
    316, 318, 332, 334, 439, // renameat2 getrandom statx rseq faccessat2
];

fn stmt(code: u16, k: u32) -> SockFilter {
    SockFilter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

fn jump(code: u16, k: u32, jt: usize, jf: usize) -> SockFilter {
    let b = |n: usize| u8::try_from(n).expect("BPF jump too far");
    SockFilter {
        code,
        jt: b(jt),
        jf: b(jf),
        k,
    }
}

/// The allowlist filter. `exe_ptr` is the address of the path string the
/// launcher passes to its single execve; execve with any other pointer (and
/// execveat, fork, clone, socket, ptrace, mount, ...) kills the process.
fn seccomp_filter(exe_ptr: u64, pid: u32) -> Vec<SockFilter> {
    let arg = |i: u32, hi: bool| 16 + 8 * i + if hi { 4 } else { 0 };
    // (syscall nr, [(arg offset, required value)]) rules with argument checks.
    let special: [(u32, Vec<(u32, u32)>); 4] = [
        (16, vec![(arg(1, false), TCGETS)]), // ioctl(fd, TCGETS)
        (
            59,
            vec![
                (arg(0, false), exe_ptr as u32),
                (arg(0, true), (exe_ptr >> 32) as u32),
            ],
        ), // execve(our exe)
        (62, vec![(arg(0, false), pid)]),    // kill(self, ..)
        (234, vec![(arg(0, false), pid)]),   // tgkill(self, ..)
    ];
    let mut f = vec![
        stmt(LD_ABS, 4), // arch
        jump(JEQ, AUDIT_ARCH_X86_64, 1, 0),
        stmt(RET, RET_KILL_PROCESS),
        stmt(LD_ABS, 0),              // nr
        jump(JGE, 0x4000_0000, 0, 1), // x32 ABI
        stmt(RET, RET_KILL_PROCESS),
    ];
    for (nr, checks) in &special {
        f.push(stmt(LD_ABS, 0));
        f.push(jump(JEQ, *nr, 0, 2 * checks.len() + 2));
        for (off, val) in checks {
            f.push(stmt(LD_ABS, *off));
            f.push(jump(JEQ, *val, 0, 2 * checks.len()));
        }
        f.push(stmt(RET, RET_ALLOW));
        f.push(stmt(RET, RET_KILL_PROCESS));
        // Mismatched checks jump to the KILL just above; fix their offsets.
        let kill = f.len() - 1;
        let base = kill - 1 - 2 * checks.len();
        for c in 0..checks.len() {
            let at = base + 2 * c + 1;
            f[at].jf = (kill - at - 1) as u8;
        }
    }
    f.push(stmt(LD_ABS, 0));
    let n = ALLOWED.len();
    for (i, nr) in ALLOWED.iter().enumerate() {
        f.push(jump(JEQ, *nr, n - i, 0)); // jt lands on RET_ALLOW
    }
    f.push(stmt(RET, RET_KILL_PROCESS));
    f.push(stmt(RET, RET_ALLOW));
    f
}

fn seccomp_available() -> bool {
    // SAFETY: PR_GET_SECCOMP has no side effects; EINVAL means no seccomp.
    unsafe { prctl(PR_GET_SECCOMP, 0usize, 0usize, 0usize, 0usize) >= 0 }
}

// -------------------------------------------------------------------- child

fn child_fail(msg: &[u8]) -> ! {
    // SAFETY: async-signal-safe raw write + _exit after fork; no allocation.
    unsafe {
        write(2, b"limits-sandbox: child setup failed: ".as_ptr(), 36);
        write(2, msg.as_ptr(), msg.len());
        _exit(127)
    }
}

struct Plan<'a> {
    limits: &'a [(i32, u64)],
    ruleset: Option<i32>,
    filter: Option<&'a SockFprog>,
    exe: &'a CString,
    argv: &'a [*const i8],
    envp: &'a [*const i8],
    /// The launcher's pid, to detect that it died before PDEATHSIG was set.
    parent: i32,
}

/// Runs between fork and exec: only syscalls, no allocation, no locks.
fn child(p: &Plan) -> ! {
    // The program must never outlive the launcher (which enforces the wall
    // limit): if the launcher is killed, the kernel SIGKILLs the program.
    // SAFETY: plain prctl/getppid. The getppid check closes the race where
    // the launcher died before the prctl took effect.
    if unsafe { prctl(PR_SET_PDEATHSIG, SIGKILL as usize, 0usize, 0usize, 0usize) } != 0 {
        child_fail(b"PR_SET_PDEATHSIG\n");
    }
    if unsafe { getppid() } != p.parent {
        // SAFETY: async-signal-safe _exit after fork.
        unsafe { _exit(128 + SIGKILL) }
    }
    for (res, val) in p.limits {
        // RLIMIT_CPU: soft limit sends SIGXCPU; hard limit (+1s) SIGKILLs a
        // program that catches SIGXCPU.
        let l = RLimit {
            cur: *val,
            max: if *res == 0 {
                val.saturating_add(1)
            } else {
                *val
            },
        };
        // SAFETY: l is a valid rlimit struct.
        if unsafe { setrlimit(*res, &l) } != 0 {
            child_fail(b"setrlimit\n");
        }
    }
    // SAFETY: plain prctl; required before seccomp/landlock as non-root.
    if unsafe { prctl(PR_SET_NO_NEW_PRIVS, 1usize, 0usize, 0usize, 0usize) } != 0 {
        child_fail(b"PR_SET_NO_NEW_PRIVS\n");
    }
    if let Some(fd) = p.ruleset {
        // SAFETY: fd is a landlock ruleset fd created by the parent.
        if unsafe { syscall(SYS_LL_RESTRICT, fd, 0u32) } != 0 {
            child_fail(b"landlock_restrict_self\n");
        }
        // SAFETY: closing our own ruleset fd once.
        unsafe { close(fd) };
    }
    if let Some(prog) = p.filter {
        // SAFETY: prog points to a filter vector that outlives this call.
        if unsafe { syscall(SYS_SECCOMP, 1u32, 0u32, prog as *const SockFprog) } != 0 {
            child_fail(b"seccomp(SET_MODE_FILTER)\n");
        }
    }
    // SAFETY: exe/argv/envp are NUL-terminated and kept alive by the parent
    // frame; exe.as_ptr() is exactly the pointer the seccomp filter allows.
    unsafe { execve(p.exe.as_ptr(), p.argv.as_ptr(), p.envp.as_ptr()) };
    child_fail(b"execve\n")
}

// ------------------------------------------------------------------- parent

fn signal_reason(sig: i32) -> &'static str {
    match sig {
        24 => "cpu limit exceeded",
        25 => "file size limit exceeded",
        31 => "forbidden syscall (seccomp)",
        11 => "segmentation fault",
        6 => "abort",
        9 => "killed: cpu hard limit or external kill",
        _ => "signal",
    }
}

/// Die of signal `s` (default action, no core file), as the child did.
fn die_of(s: i32) -> ! {
    let no_core = RLimit { cur: 0, max: 0 };
    // SAFETY: plain libc calls on our own process; SIG_DFL is 0.
    unsafe {
        setrlimit(4, &no_core); // RLIMIT_CORE
        signal(s, 0);
        raise(s);
    }
    // Signals whose default action is to be ignored (none of the child's
    // fatal ones, but be safe) fall back to the usual exit code.
    std::process::exit(128 + s)
}

fn main() {
    let o = parse(std::env::args().skip(1).collect());
    let exe = resolve(&o.prog[0]);

    let abi = landlock_abi();
    let have_seccomp = seccomp_available();
    let mut degraded = Vec::new();
    if abi < 1 {
        degraded.push(format!(
            "Landlock unavailable (errno {}): filesystem/network NOT restricted",
            errno()
        ));
    }
    if !have_seccomp {
        degraded.push("seccomp unavailable: syscalls NOT restricted".to_string());
    }
    for d in &degraded {
        eprintln!("limits-sandbox: WARNING: {d}");
    }
    if !degraded.is_empty() && !o.allow_degraded {
        die("refusing to run with a degraded sandbox (pass --allow-degraded to override)");
    }
    let ruleset = if abi >= 1 {
        Some(landlock_ruleset(abi, &o, &exe).unwrap_or_else(|e| die(&e)))
    } else {
        None
    };

    let mb = |n: u64| n.saturating_mul(1 << 20);
    let limits = [
        (0, o.cpu_sec),      // RLIMIT_CPU
        (9, mb(o.mem_mb)),   // RLIMIT_AS
        (1, mb(o.fsize_mb)), // RLIMIT_FSIZE
        (6, o.nproc),        // RLIMIT_NPROC
        (7, 64),             // RLIMIT_NOFILE
        (4, 0),              // RLIMIT_CORE
    ];
    let exe_c = cstr(exe.as_os_str().as_bytes());
    let args: Vec<CString> = o.prog.iter().map(|a| cstr(a.as_bytes())).collect();
    let mut argv: Vec<*const i8> = args.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    let env: Vec<CString> = child_env(&o.env).iter().map(|e| cstr(e)).collect();
    let mut envp: Vec<*const i8> = env.iter().map(|e| e.as_ptr()).collect();
    envp.push(std::ptr::null());

    // SAFETY: getpid has no preconditions.
    let parent = unsafe { getpid() };
    let start = Instant::now();
    // SAFETY: the launcher is single-threaded; the child only runs `child`.
    let pid = unsafe { fork() };
    if pid < 0 {
        die("fork failed");
    }
    if pid == 0 {
        // SAFETY: getpid has no preconditions. Allocating the filter after
        // fork is sound: the launcher is single-threaded, so no other thread
        // can hold the allocator lock. The pid is needed for kill/tgkill.
        let me = unsafe { getpid() } as u32;
        let f = seccomp_filter(exe_c.as_ptr() as u64, me);
        let prog = SockFprog {
            len: f.len() as u16,
            filter: f.as_ptr(),
        };
        child(&Plan {
            limits: &limits,
            ruleset,
            filter: have_seccomp.then_some(&prog),
            exe: &exe_c,
            argv: &argv,
            envp: &envp,
            parent,
        });
    }
    if let Some(fd) = ruleset {
        // SAFETY: closing the parent's copy of the ruleset fd.
        unsafe { close(fd) };
    }

    let deadline = Duration::from_secs(o.wall_sec);
    let mut status = 0i32;
    let mut timed_out = false;
    loop {
        // SAFETY: status is a valid out-pointer; pid is our child.
        let r = unsafe { waitpid(pid, &mut status, if timed_out { 0 } else { WNOHANG }) };
        if r == pid {
            break;
        }
        if r < 0 && errno() != 4 {
            die("waitpid failed");
        }
        if !timed_out && start.elapsed() >= deadline {
            // SAFETY: sending SIGKILL to our own child.
            unsafe { kill(pid, SIGKILL) };
            timed_out = true;
            continue;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let code = if timed_out {
        eprintln!("limits-sandbox: timeout (wall limit {}s)", o.wall_sec);
        128 + SIGKILL
    } else if status & 0x7f == 0 {
        let c = (status >> 8) & 0xff;
        if !o.quiet {
            eprintln!("limits-sandbox: exit {c}");
        }
        c
    } else {
        let s = status & 0x7f;
        if o.quiet {
            die_of(s);
        }
        eprintln!(
            "limits-sandbox: killed by signal {s} ({})",
            signal_reason(s)
        );
        128 + s
    };
    std::process::exit(code)
}
