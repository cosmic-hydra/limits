//! CLI boundary regressions. The Unix age double below checks CLI plumbing,
//! never cryptography; real authenticated age integration remains in sealed.rs.
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        for _ in 0..100 {
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("limits-swe07-{}-{id}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("cannot create test directory: {e}"),
            }
        }
        panic!("cannot allocate test directory");
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn command(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lim"));
    command.args(args);
    command
}
fn invoke(mut command: Command, source: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    if let Err(e) = stdin.write_all(source) {
        assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe);
    }
    drop(stdin);
    child.wait_with_output().unwrap()
}
fn response(output: &Output, status: i32) -> &str {
    assert_eq!(output.status.code(), Some(status), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let json = std::str::from_utf8(&output.stdout).unwrap();
    assert!(json.starts_with("{\"schema_version\":1,"), "{json}");
    assert!(json.ends_with("}\n"), "{json}");
    assert_eq!(json.lines().count(), 1, "{json}");
    json
}

#[test]
fn invalid_flags_fail_before_reading_or_executing_stdin() {
    for args in [
        vec!["run", "-", "--engine", "vm", "--engine", "tree", "--json"],
        vec!["run", "-", "--max-steps", "-1", "--json"],
        vec!["run", "-", "--max-depth", "18446744073709551616", "--json"],
        vec!["run", "-", "--timeout-ms", "no", "--json"],
        vec!["run", "-", "--allow-builtin", "unknown", "--json"],
        vec!["run", "-", "--sandbox", "invalid", "--json"],
        vec!["check", "-", "--max-steps", "1", "--json"],
        vec!["symbols", "-", "--limit", "0"],
        vec!["symbols", "-", "--limit", "10001"],
        vec!["definition", "-", "--line", "1"],
        vec!["references", "-", "--line", "1", "--col", "0"],
        vec!["index", "-", "--prefix", "x"],
        vec!["capabilities", "extra.lim"],
        vec!["seal", "-", "--recipient", "age1invalid", "--json"],
        vec!["unseal", "-", "--out", "new.lim", "--json"],
        vec!["run", "one.lim", "two.lim", "--json"],
        vec!["run", "-", "--json", "--json"],
        vec!["run", "-", "--json", "--max-steps"],
    ] {
        let output = invoke(command(&args), b"print(\"must not execute\");");
        let json = response(&output, 2);
        assert!(json.contains("\"code\":\"LIM_USAGE\""), "{args:?}: {json}");
        assert!(json.contains("\"line\":null,\"col\":null"), "{json}");
        assert!(!json.contains("must not execute"), "{json}");
    }
}

#[test]
fn builtin_denials_apply_after_allowlists_and_profiles_in_any_order() {
    for engine in ["tree", "vm"] {
        for flags in [
            [
                "--deny-builtin",
                "print",
                "--allow-builtin",
                "print",
                "--sandbox",
                "unrestricted",
            ],
            [
                "--sandbox",
                "pure",
                "--allow-builtin",
                "print",
                "--deny-builtin",
                "print",
            ],
        ] {
            let mut args = vec!["run", "-", "--json", "--engine", engine];
            args.extend(flags);
            let output = invoke(command(&args), b"let emit=print;emit(1);");
            let json = response(&output, 1);
            assert!(json.contains("sandbox denied builtin 'print'"), "{json}");
            assert!(json.contains("\"stdout\":\"\""), "{json}");
        }
    }
}

#[test]
fn json_capture_keeps_its_default_budget_with_unrestricted_profile() {
    let source = b"let text=\"x\";for i in 0..21 {text=text+text;}print(text);";
    for engine in ["tree", "vm"] {
        let output = invoke(
            command(&[
                "run",
                "-",
                "--json",
                "--engine",
                engine,
                "--sandbox",
                "unrestricted",
            ]),
            source,
        );
        let json = response(&output, 1);
        assert!(
            json.contains("output limit exceeded (1048576 bytes)"),
            "{json}"
        );
        assert!(json.contains("\"stdout\":\"\""), "{json}");
    }
}

#[test]
fn source_size_boundary_precedes_utf8_decoding_for_stdin_and_ciphertext() {
    let mut source = vec![b' '; limits::lexer::MAX_SOURCE_BYTES];
    let output = invoke(command(&["check", "-", "--json"]), &source);
    assert!(response(&output, 0).contains("\"ok\":true"));
    // The capped read splits this scalar; size still takes precedence over UTF-8.
    source.extend_from_slice("é".as_bytes());
    let output = invoke(command(&["run", "-", "--json", "--engine", "vm"]), &source);
    let json = response(&output, 1);
    assert!(json.contains("\"code\":\"LIM_LEX\""), "{json}");
    assert!(json.contains("\"engine\":\"vm\""), "{json}");

    let temp = Temp::new();
    let path = temp.0.join("oversized.lim.age");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(limits::sealed::MAX_CIPHERTEXT_BYTES as u64 + 1)
        .unwrap();
    let output = invoke(
        command(&[
            "run",
            path.to_str().unwrap(),
            "--identity",
            "private-identity-path",
            "--json",
        ]),
        b"",
    );
    let json = response(&output, 2);
    assert!(json.contains("\"code\":\"LIM_HOST\""), "{json}");
    assert!(json.contains("\"engine\":null"), "{json}");
    assert!(!json.contains("private-identity-path"), "{json}");
}

#[test]
fn invalid_utf8_stdin_is_a_host_error_without_program_execution() {
    for args in [
        vec!["run", "-", "--json"],
        vec!["check", "-", "--json"],
        vec!["index", "-"],
    ] {
        let output = invoke(command(&args), b"print(1);\xff");
        let json = response(&output, 2);
        assert!(json.contains("source is not valid UTF-8"), "{json}");
        assert!(json.contains("\"code\":\"LIM_HOST\""), "{json}");
        assert!(json.contains("\"line\":null,\"col\":null"), "{json}");
    }
}

#[cfg(any(unix, windows))]
#[test]
fn invalid_unicode_arguments_use_the_usage_error_contract() {
    #[cfg(unix)]
    let invalid = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(b"private-argument-\xff.lim".to_vec())
    };
    #[cfg(windows)]
    let invalid = {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[b'x' as u16, 0xd800])
    };
    for args in [vec!["run", "--json"], vec!["capabilities"]] {
        let mut cmd = command(&args);
        cmd.arg(&invalid);
        let output = invoke(cmd, b"");
        let json = response(&output, 2);
        assert!(json.contains("\"code\":\"LIM_USAGE\""), "{json}");
        assert!(json.contains("arguments must be valid UTF-8"), "{json}");
        assert!(!json.contains("private-argument"), "{json}");
    }
}

#[cfg(target_os = "linux")]
fn failed_sink() -> std::fs::File {
    std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap()
}

#[cfg(target_os = "linux")]
#[test]
fn failed_stderr_preserves_program_and_host_exit_codes() {
    for (args, source, status) in [
        (vec!["run"], "", 2),
        (vec!["run", "missing-source.lim"], "", 2),
        (vec!["check", "-"], "unknown;", 1),
        (vec!["run", "-"], "print(1/0);", 1),
    ] {
        let mut child = command(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(failed_sink())
            .spawn()
            .unwrap();
        let _ = child.stdin.take().unwrap().write_all(source.as_bytes());
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(status), "{args:?}");
        assert!(output.stdout.is_empty());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn failed_control_plane_stdout_is_a_host_error_even_when_stderr_also_fails() {
    for args in [
        vec!["--help"],
        vec!["capabilities"],
        vec!["run", "missing-source.lim", "--json"],
        vec!["run", "--json"],
        vec!["check", "-", "--json"],
    ] {
        let output = command(&args)
            .stdin(Stdio::null())
            .stdout(failed_sink())
            .stderr(failed_sink())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[cfg(unix)]
#[test]
fn encrypted_cli_plumbing_refuses_existing_destinations_and_suppresses_failed_plaintext() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let temp = Temp::new();
    let executable = temp.0.join("age");
    std::fs::write(
        &executable,
        r#"#!/bin/sh
if [ "$LIMITS_SWE07_FAIL" = 1 ]; then
    printf 'unauthenticated-private-source'
    printf 'private-child-stderr' >&2
    exit 7
fi
case "$1" in
    --encrypt) printf 'age-encryption.org/v1\n'; /bin/cat ;;
    --decrypt) IFS= read -r header; /bin/cat ;;
    *) exit 9 ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let identity = temp.0.join("identity.key");
    std::fs::write(&identity, format!("AGE-SECRET-KEY-1{}\n", "Q".repeat(58))).unwrap();
    let recipient = format!("age1{}", "q".repeat(58));
    let ciphertext = temp.0.join("source.lim.age");
    let source = "print(\"agent é\");\n";
    let make = |args: &[&str]| {
        let mut cmd = command(args);
        cmd.env("PATH", &temp.0).env_remove("LIMITS_SWE07_FAIL");
        cmd
    };
    let seal = [
        "seal",
        "-",
        "--recipient",
        &recipient,
        "--out",
        ciphertext.to_str().unwrap(),
        "--json",
    ];
    let output = invoke(make(&seal), source.as_bytes());
    assert!(response(&output, 0).contains("\"format\":\"age-v1\""));
    let sealed = std::fs::read(&ciphertext).unwrap();
    let output = invoke(make(&seal), b"replacement must not overwrite");
    assert!(response(&output, 2).contains("existing files are never overwritten"));
    assert_eq!(std::fs::read(&ciphertext).unwrap(), sealed);

    let plain = temp.0.join("restored.lim");
    let unseal = [
        "unseal",
        ciphertext.to_str().unwrap(),
        "--identity",
        identity.to_str().unwrap(),
        "--out",
        plain.to_str().unwrap(),
        "--json",
    ];
    let output = invoke(make(&unseal), b"");
    assert!(response(&output, 0).contains("\"format\":\"utf-8-limits\""));
    assert_eq!(std::fs::read_to_string(&plain).unwrap(), source);
    for path in [&ciphertext, &plain] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    for existing in [
        "regular.lim",
        "symlink.lim",
        "directory.lim",
        "dangling.lim",
    ] {
        let target = temp.0.join(existing);
        match existing {
            "regular.lim" => std::fs::write(&target, "preserve existing data").unwrap(),
            "symlink.lim" => symlink(&plain, &target).unwrap(),
            "directory.lim" => std::fs::create_dir(&target).unwrap(),
            _ => symlink(temp.0.join("absent.lim"), &target).unwrap(),
        }
        let mut args = unseal;
        args[5] = target.to_str().unwrap();
        let output = invoke(make(&args), b"");
        assert!(response(&output, 2).contains("existing files are never overwritten"));
        assert_eq!(std::fs::read_to_string(&plain).unwrap(), source);
        if existing == "regular.lim" {
            assert_eq!(
                std::fs::read_to_string(target).unwrap(),
                "preserve existing data"
            );
        }
    }
    let failed_output = temp.0.join("failed.lim");
    let mut args = unseal;
    args[5] = failed_output.to_str().unwrap();
    let mut cmd = make(&args);
    cmd.env("LIMITS_SWE07_FAIL", "1");
    let output = invoke(cmd, b"");
    let json = response(&output, 2);
    assert!(json.contains("age operation failed"), "{json}");
    assert!(!json.contains("unauthenticated-private-source"), "{json}");
    assert!(!json.contains("private-child-stderr"), "{json}");
    assert!(
        !json.contains(&identity.to_string_lossy().to_string()),
        "{json}"
    );
    assert!(!failed_output.exists());
}
