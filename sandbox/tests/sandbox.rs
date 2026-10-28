//! End-to-end tests: compile small C probes and run them under the sandbox.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

const PROBE: &str = r#"
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/socket.h>
int main(int argc, char **argv) {
    const char *m = argv[1];
    if (!strcmp(m, "read")) {
        FILE *f = fopen(argv[2], "r");
        if (!f) { printf("err %d\n", errno); return 3; }
        char buf[64] = {0};
        size_t n = fread(buf, 1, sizeof buf - 1, f);
        printf("ok %zu\n", n);
        return 0;
    }
    if (!strcmp(m, "write")) {
        FILE *f = fopen(argv[2], "w");
        if (!f) { printf("err %d\n", errno); return 3; }
        fputs("x", f); fclose(f); printf("ok\n");
        return 0;
    }
    if (!strcmp(m, "socket")) { int s = socket(AF_INET, SOCK_STREAM, 0); printf("socket %d\n", s); return 0; }
    if (!strcmp(m, "fork")) { pid_t p = fork(); printf("fork %d\n", p); return 0; }
    if (!strcmp(m, "exec")) {
        char *a[] = {"/bin/sh", "-c", "echo pwned", 0};
        execv("/bin/sh", a); printf("exec failed %d\n", errno); return 0;
    }
    if (!strcmp(m, "mem")) {
        size_t n = (size_t)atol(argv[2]) << 20;
        char *p = malloc(n);
        if (!p) { printf("alloc failed\n"); return 4; }
        memset(p, 1, n); printf("alloc ok\n");
        return 0;
    }
    if (!strcmp(m, "cpu")) { volatile unsigned long x = 0; for (;;) x++; }
    if (!strcmp(m, "env")) { extern char **environ; for (char **e = environ; *e; e++) printf("ENV %s\n", *e); return 0; }
    if (!strcmp(m, "sleep")) { sleep(60); return 0; }
    return 2;
}
"#;

fn scratch() -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sandbox-tests");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(d).unwrap()
}

fn probe() -> &'static Path {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| {
        let d = scratch();
        std::fs::write(d.join("probe.c"), PROBE).unwrap();
        let out = d.join("probe");
        let st = Command::new("cc")
            .arg("-O1")
            .arg("-o")
            .arg(&out)
            .arg(d.join("probe.c"))
            .status();
        assert!(
            st.expect("cc not found").success(),
            "probe failed to compile"
        );
        out
    })
}

fn sandbox(opts: &[&str], prog: &Path, args: &[&str]) -> (i32, String, String) {
    let o: Output = Command::new(env!("CARGO_BIN_EXE_limits-sandbox"))
        .args(opts)
        .arg("--")
        .arg(prog)
        .args(args)
        .output()
        .unwrap();
    let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (o.status.code().unwrap_or(-1), s(&o.stdout), s(&o.stderr))
}

fn run_env(opts: &[&str], vars: &[(&str, &str)]) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_limits-sandbox"))
        .envs(vars.iter().copied())
        .args(opts)
        .arg("--")
        .arg(probe())
        .arg("env")
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn secrets_in_environment_are_not_passed() {
    let out = run_env(
        &[],
        &[
            ("SECRET_TOKEN", "hunter2"),
            ("GH_TOKEN", "x"),
            ("AWS_SECRET_ACCESS_KEY", "y"),
            ("LC_ALL", "C"),
        ],
    );
    assert!(
        !out.contains("hunter2") && !out.contains("GH_TOKEN") && !out.contains("AWS_"),
        "{out}"
    );
    assert!(
        out.contains("ENV LC_ALL=C") && out.contains("ENV PATH="),
        "{out}"
    );
}

#[test]
fn env_option_passes_or_sets_named_vars() {
    let out = run_env(
        &["--env", "KEEP", "--env", "SET=v=1", "--env", "PATH=/x"],
        &[("KEEP", "k"), ("DROP", "d")],
    );
    assert!(
        out.contains("ENV KEEP=k\n") && out.contains("ENV SET=v=1\n"),
        "{out}"
    );
    assert!(
        out.contains("ENV PATH=/x\n") && !out.contains("DROP"),
        "{out}"
    );
}

fn run(opts: &[&str], args: &[&str]) -> (i32, String, String) {
    sandbox(opts, probe(), args)
}

fn dir(name: &str) -> String {
    let d = scratch().join(name);
    std::fs::create_dir_all(&d).unwrap();
    d.to_string_lossy().into_owned()
}

const SIGSYS_KILL: &str = "limits-sandbox: killed by signal 31 (forbidden syscall (seccomp))";

#[test]
fn allowed_read_works() {
    let d = dir("ro");
    std::fs::write(format!("{d}/f.txt"), "hello").unwrap();
    let (code, out, err) = run(&["--read", &d], &["read", &format!("{d}/f.txt")]);
    assert_eq!((code, out.as_str()), (0, "ok 5\n"), "{err}");
    assert!(err.contains("limits-sandbox: exit 0"), "{err}");
}

#[test]
fn read_outside_allowlist_is_denied() {
    let d = dir("ro2");
    let (code, out, _) = run(&["--read", &d], &["read", "/etc/passwd"]);
    assert_eq!(code, 3);
    assert!(
        out == "err 13\n" || out == "err 1\n",
        "expected EACCES/EPERM, got {out}"
    );
}

#[test]
fn write_outside_allowlist_is_denied() {
    let (rw, ro) = (dir("rw"), dir("ro3"));
    let (code, out, _) = run(
        &["--read", &ro, "--write", &rw],
        &["write", &format!("{rw}/ok.txt")],
    );
    assert_eq!((code, out.as_str()), (0, "ok\n"));
    let (code, out, _) = run(
        &["--read", &ro, "--write", &rw],
        &["write", &format!("{ro}/no.txt")],
    );
    assert_eq!(code, 3);
    assert!(out == "err 13\n" || out == "err 1\n", "got {out}");
    assert!(!Path::new(&format!("{ro}/no.txt")).exists());
}

#[test]
fn socket_kills() {
    let (code, out, err) = run(&[], &["socket"]);
    assert_eq!((code, out.as_str()), (128 + 31, ""));
    assert!(err.contains(SIGSYS_KILL), "{err}");
}

#[test]
fn fork_kills() {
    let (code, out, err) = run(&[], &["fork"]);
    assert_eq!((code, out.as_str()), (128 + 31, ""));
    assert!(err.contains(SIGSYS_KILL), "{err}");
}

#[test]
fn exec_shell_kills() {
    let (code, out, err) = run(&[], &["exec"]);
    assert_eq!((code, out.as_str()), (128 + 31, ""), "{err}");
    assert!(err.contains(SIGSYS_KILL), "{err}");
}

#[test]
fn memory_limit_fails_allocation() {
    let (code, out, _) = run(&["--mem-mb", "64"], &["mem", "32"]);
    assert_eq!((code, out.as_str()), (0, "alloc ok\n"));
    let (code, out, _) = run(&["--mem-mb", "64"], &["mem", "256"]);
    assert_eq!((code, out.as_str()), (4, "alloc failed\n"));
}

#[test]
fn cpu_limit_kills() {
    let (code, _, err) = run(&["--cpu-sec", "1", "--wall-sec", "20"], &["cpu"]);
    assert_eq!(code, 128 + 24, "{err}");
    assert!(
        err.contains("killed by signal 24 (cpu limit exceeded)"),
        "{err}"
    );
}

#[test]
fn wall_timeout_kills() {
    let t = std::time::Instant::now();
    let (code, _, err) = run(&["--wall-sec", "1"], &["sleep"]);
    assert_eq!(code, 128 + 9);
    assert!(err.contains("limits-sandbox: timeout"), "{err}");
    assert!(t.elapsed().as_secs() < 10);
}

#[test]
fn exit_code_is_propagated() {
    let (code, _, err) = run(&[], &["bogus-mode"]);
    assert_eq!(code, 2);
    assert!(err.contains("limits-sandbox: exit 2"), "{err}");
}

#[test]
fn real_limits_programs_run_inside() {
    let limits = Path::new(env!("CARGO_MANIFEST_DIR")).join("../native/target/release/limits");
    if !limits.exists() {
        eprintln!("SKIP: {} not built", limits.display());
        return;
    }
    let d = dir("lim");
    let hello = format!("{d}/hello.lim");
    std::fs::write(&hello, "fn main() {\n    print(\"hello, world\");\n}\n").unwrap();
    let st = Command::new(&limits)
        .args(["build", &hello, "-o", &format!("{d}/hello")])
        .status()
        .unwrap();
    assert!(st.success());
    let (code, out, err) = sandbox(&[], Path::new(&format!("{d}/hello")), &[]);
    assert_eq!((code, out.as_str()), (0, "hello, world\n"), "{err}");

    let src = format!("{d}/rd.lim");
    std::fs::write(
        &src,
        "uses fs.read, args;\nfn main() { print(read_file(args()[0])); }\n",
    )
    .unwrap();
    std::fs::write(format!("{d}/in.txt"), "file contents").unwrap();
    let allow = format!("fs.read={d}");
    let st = Command::new(&limits)
        .args(["build", &src, "-o", &format!("{d}/rd"), "--allow", &allow])
        .status()
        .unwrap();
    assert!(st.success());
    let rd = PathBuf::from(format!("{d}/rd"));
    let (code, out, err) = sandbox(&["--read", &d], &rd, &[&format!("{d}/in.txt")]);
    assert_eq!((code, out.trim_end()), (0, "file contents"), "{err}");
    // Without --read the kernel refuses even though the program was built
    // with the fs.read grant: the sandbox is an independent layer.
    let (code, out, err) = sandbox(&[], &rd, &[&format!("{d}/in.txt")]);
    assert_eq!((code, out.as_str()), (1, ""), "{err}");
    assert!(err.contains("Permission denied"), "{err}");
}
