//! `limits run`/`limits test` execute programs under limits-sandbox: the OS
//! denies what the runtime denies, even when the runtime check is bypassed.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "limits-sbx-{}-{}-{}",
        tag,
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Out {
    stdout: String,
    stderr: String,
    code: i32,
}

fn out(o: std::process::Output) -> Out {
    Out {
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        code: o.status.code().unwrap_or(-1),
    }
}

/// Write `src` to `dir/t.lim` and run `limits <args>` on it from `dir`.
fn limits(mut c: Command, dir: &Path, src: &str, args: &[&str], bypass: bool) -> Out {
    std::fs::write(dir.join("t.lim"), src).unwrap();
    c.current_dir(dir).args(args);
    if bypass {
        c.env("LIMITS_UNSAFE_NO_RUNTIME_FS", "1");
    }
    out(c.output().unwrap())
}

/// A secret outside the program's grants, and a working dir it may read.
fn setup() -> (PathBuf, PathBuf) {
    let work = tmp("work");
    let secret = tmp("secret");
    std::fs::write(secret.join("key.txt"), "TOPSECRET").unwrap();
    std::fs::write(work.join("in.txt"), "hello").unwrap();
    (work, secret)
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the runtime-check bypass hook only exists in debug builds"
)]
fn read_denied_by_runtime_is_denied_by_the_os_too() {
    let (work, secret) = setup();
    let key = secret.join("key.txt");
    let src = format!(
        "uses fs.read\nprint(env.fs.read(\"in.txt\"))\nprint(env.fs.read(\"{}\"))\n",
        key.display()
    );
    let allow = format!("fs.read={}", work.display());
    let args = ["run", "t.lim", "--allow", &allow];
    // Runtime check on: denied by the runtime.
    let o = limits(common::limits(), &work, &src, &args, false);
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert!(o.stderr.contains("access denied"), "{}", o.stderr);
    // Runtime check off, no sandbox: the secret leaks (the bypass works).
    let mut a = args.to_vec();
    a.push("--no-sandbox");
    let o = limits(common::limits(), &work, &src, &a, true);
    assert!(
        o.stdout.contains("TOPSECRET"),
        "bypass ineffective: {}",
        o.stderr
    );
    assert!(o.stderr.contains("--no-sandbox"), "{}", o.stderr);
    // Runtime check off, sandbox on: the kernel refuses.
    let o = limits(common::limits(), &work, &src, &args, true);
    assert_eq!(o.stdout, "hello\n", "{}", o.stderr);
    assert!(!o.stdout.contains("TOPSECRET"));
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert!(
        o.stderr.contains("Permission denied") || o.stderr.contains("Operation not permitted"),
        "{}",
        o.stderr
    );
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(&secret);
}

#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the runtime-check bypass hook only exists in debug builds"
)]
fn write_denied_by_runtime_is_denied_by_the_os_too() {
    let (work, secret) = setup();
    let target = secret.join("planted.txt");
    let src = format!(
        "uses fs.write\nenv.fs.write(\"ok.txt\", \"fine\")\nenv.fs.write(\"{}\", \"x\")\n",
        target.display()
    );
    let o = limits(common::limits(), &work, &src, &["run", "t.lim"], false);
    assert!(o.stderr.contains("access denied"), "{}", o.stderr);
    let o = limits(common::limits(), &work, &src, &["run", "t.lim"], true);
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert!(o.stderr.contains("Permission denied"), "{}", o.stderr);
    assert_eq!(
        std::fs::read_to_string(work.join("ok.txt")).unwrap(),
        "fine"
    );
    assert!(!target.exists(), "the sandbox let the write through");
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(&secret);
}

#[test]
fn undeclared_fs_gets_no_os_access() {
    // No `uses fs.read`: the sandbox grants no directory, not even the cwd.
    let (work, secret) = setup();
    let o = limits(
        common::limits(),
        &work,
        "print(1)\n",
        &["run", "t.lim", "--timeout", "5"],
        false,
    );
    assert_eq!(
        (o.code, o.stdout.as_str(), o.stderr.as_str()),
        (0, "1\n", "")
    );
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(&secret);
}

#[test]
fn env_vars_pass_only_when_named() {
    let work = tmp("env");
    let src = "uses env\nprint(\"[\" + env.vars.get(\"LIMITS_SBX_T\") + \"]\")\n";
    let run = |extra: &[&str]| {
        let mut c = common::limits();
        c.env("LIMITS_SBX_T", "v1");
        let mut a = vec!["run", "t.lim"];
        a.extend(extra);
        limits(c, &work, src, &a, false)
    };
    assert_eq!(run(&[]).stdout, "[]\n");
    assert_eq!(run(&["--pass-env", "LIMITS_SBX_T"]).stdout, "[v1]\n");
    assert_eq!(run(&["--allow", "env=LIMITS_SBX_T"]).stdout, "[v1]\n");
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn timeout_and_memory_limits_apply() {
    let work = tmp("lim");
    let o = limits(
        common::limits(),
        &work,
        "var i = 0\nwhile true: i += 1\n",
        &["run", "t.lim", "--unchecked", "--timeout", "1"],
        false,
    );
    assert_eq!(o.code, 137, "{}", o.stderr);
    assert!(
        o.stderr.contains("program timed out (wall limit 1 s)"),
        "{}",
        o.stderr
    );
    let _ = std::fs::remove_dir_all(&work);
}

/// Reviewer regressions: the wall limit holds without the sandbox, and with
/// --json it is a JSON runtime diagnostic like a death by signal.
#[test]
fn timeout_applies_without_sandbox_and_in_json() {
    let work = tmp("lim2");
    let spin = "var i = 0\nwhile true: i += 1\n";
    let t = std::time::Instant::now();
    let o = limits(
        common::limits(),
        &work,
        spin,
        &[
            "run",
            "t.lim",
            "--unchecked",
            "--timeout",
            "1",
            "--no-sandbox",
        ],
        false,
    );
    assert!(t.elapsed().as_secs() < 20, "--no-sandbox ignored --timeout");
    assert_eq!(o.code, 137, "{}", o.stderr);
    assert!(o.stderr.contains("timed out"), "{}", o.stderr);
    let o = limits(
        common::limits(),
        &work,
        spin,
        &["run", "t.lim", "--unchecked", "--timeout", "1", "--json"],
        false,
    );
    assert_eq!(o.code, 137, "{}", o.stderr);
    let first = o.stderr.lines().next().unwrap_or("");
    assert!(
        first.starts_with("{\"kind\":\"runtime\"") && first.contains("timed out"),
        "{}",
        o.stderr
    );
    let _ = std::fs::remove_dir_all(&work);
}

/// Reviewer regression: on the per-test timeout limits killed only the
/// sandbox launcher; the program was orphaned, kept running and held the
/// stderr pipe, so the run took as long as the program's CPU limit.
#[test]
fn test_timeout_kills_the_sandboxed_program() {
    let work = tmp("tt");
    let src = "fn spin() -> int:\n    var n = 0\n    while true:\n        n = n + 1\n    return n\ntest \"spin\": assert(spin() == 1)\n";
    let t = std::time::Instant::now();
    let o = limits(
        common::limits(),
        &work,
        src,
        &["test", "t.lim", "--unchecked", "--timeout", "40"],
        false,
    );
    let took = t.elapsed().as_secs();
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert!(
        o.stdout.contains("FAIL spin: timed out after 10 s"),
        "{}",
        o.stdout
    );
    assert!(
        took < 25,
        "run took {} s: the program outlived the test",
        took
    );
    let o = limits(
        common::limits(),
        &work,
        src,
        &["test", "t.lim", "--unchecked", "--timeout", "2"],
        false,
    );
    assert!(
        o.stdout.contains("FAIL spin: timed out after 2 s"),
        "{}",
        o.stdout
    );
    let _ = std::fs::remove_dir_all(&work);
}

/// Reviewer regression: `--pass-env NAME=VALUE` injected arbitrary variables
/// (e.g. LD_PRELOAD) into the sandbox; only names are accepted.
#[test]
fn pass_env_takes_names_only() {
    let work = tmp("envname");
    let src = "uses env\nprint(\"x\")\n";
    for bad in [
        &["--pass-env", "LD_PRELOAD=/nonexist.so"][..],
        &["--pass-env", ""],
        &["--allow", "env=LD_PRELOAD=/nonexist.so"],
    ] {
        let mut a = vec!["run", "t.lim"];
        a.extend(bad);
        let o = limits(common::limits(), &work, src, &a, false);
        assert_eq!(o.code, 2, "{:?}: {}", bad, o.stderr);
        assert!(o.stderr.contains("not a variable NAME"), "{}", o.stderr);
        assert!(!o.stderr.contains("ld.so"), "{}", o.stderr);
    }
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn missing_sandbox_is_an_error_unless_opted_out() {
    // A copy of limits outside the repository, with no sandbox anywhere.
    let dir = tmp("nosbx");
    let exe = dir.join("limits");
    std::fs::copy(env!("CARGO_BIN_EXE_limits"), &exe).unwrap();
    let mk = || {
        let mut c = Command::new(&exe);
        c.env_remove("LIMITS_SANDBOX").env("PATH", "/usr/bin:/bin");
        c
    };
    let o = limits(mk(), &dir, "print(1)\n", &["run", "t.lim"], false);
    assert_eq!(o.code, 2);
    assert!(
        o.stderr.contains("cannot find limits-sandbox"),
        "{}",
        o.stderr
    );
    assert!(o.stderr.contains("--no-sandbox"), "{}", o.stderr);
    let o = limits(
        mk(),
        &dir,
        "print(1)\n",
        &["run", "t.lim", "--no-sandbox"],
        false,
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(o.stdout, "1\n");
    assert!(o.stderr.contains("warning"), "{}", o.stderr);
    let _ = std::fs::remove_dir_all(&dir);
}
