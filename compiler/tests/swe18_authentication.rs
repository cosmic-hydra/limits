use limits::sealed::{Age, SealedError};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const HEADER: &[u8] = b"age-encryption.org/v1\n";

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        for _ in 0..100 {
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("limits-swe18-{}-{id}", std::process::id()));
            let builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            let mut builder = builder;
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => panic!("cannot create test directory"),
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

fn assert_sanitized(error: SealedError, forbidden: &[&str]) {
    let displayed = error.to_string();
    let debugged = format!("{error:?}");
    for value in forbidden {
        assert!(!displayed.contains(value));
        assert!(!debugged.contains(value));
    }
    assert!(std::error::Error::source(&error).is_none());
}

#[test]
fn armor_and_non_binary_envelopes_are_rejected_before_identity_or_tool_access() {
    let temp = Temp::new();
    let age = Age::with_executable(temp.0.join("missing-secret-tool-path"));
    let identity = temp.0.join("missing-secret-identity-path");
    for ciphertext in [
        b"-----BEGIN AGE ENCRYPTED FILE-----\nYWdlLWVuY3J5cHRpb24ub3JnL3YxCg==\n-----END AGE ENCRYPTED FILE-----\n".as_slice(),
        b"age-encryption.org/v1\r\n".as_slice(),
        b"\xef\xbb\xbfage-encryption.org/v1\n".as_slice(),
        b"age-encryption.org/v2\n".as_slice(),
        b"age-encryption.org/v1\0\n".as_slice(),
        b"".as_slice(),
    ] {
        let error = age.open(ciphertext, &identity).unwrap_err();
        assert_eq!(error, SealedError::InvalidCiphertext);
        assert_sanitized(error, &["missing-secret-tool", "missing-secret-identity"]);
    }
}

#[test]
fn decryption_spawn_errors_do_not_reveal_canonical_identity_or_executable_paths() {
    let temp = Temp::new();
    let identity = temp.0.join("private-native-identity.key");
    let native_shape = format!("AGE-SECRET-KEY-1{}\n", "Q".repeat(58));
    std::fs::write(&identity, &native_shape).unwrap();
    let executable = temp.0.join("private-tool-location");
    let error = Age::with_executable(&executable)
        .open(HEADER, &identity)
        .unwrap_err();
    assert_eq!(error, SealedError::ToolUnavailable);
    assert_sanitized(
        error,
        &[
            "private-native-identity",
            "private-tool-location",
            &native_shape,
        ],
    );

    // A present file without executable permission is a separate spawn failure
    // from a missing tool, and must still use the same sanitized error boundary.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&executable, b"not an executable").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o600)).unwrap();
        let error = Age::with_executable(&executable)
            .open(HEADER, &identity)
            .unwrap_err();
        assert_eq!(error, SealedError::OperationFailed);
        assert_sanitized(error, &["private-native-identity", "private-tool-location"]);
    }
}

fn tools() -> (PathBuf, PathBuf) {
    (
        std::env::var_os("LIMITS_AGE_BINARY")
            .unwrap_or_else(|| "age".into())
            .into(),
        std::env::var_os("LIMITS_AGE_KEYGEN")
            .unwrap_or_else(|| "age-keygen".into())
            .into(),
    )
}

fn generate_identity(keygen: &Path, path: &Path) -> String {
    let output = Command::new(keygen)
        .arg("-o")
        .arg(path)
        .output()
        .expect("real age-keygen is required for this explicit integration test");
    assert!(output.status.success(), "age-keygen failed");
    let output = Command::new(keygen).arg("-y").arg(path).output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().into()
}

#[test]
#[ignore = "requires real age and age-keygen; run cargo test --test swe18_authentication -- --ignored"]
fn real_age_validates_native_checksums_and_accepts_multiple_native_identities() {
    let (executable, keygen) = tools();
    let age = Age::with_executable(executable);
    let temp = Temp::new();
    let first = temp.0.join("first-private-identity.key");
    let second = temp.0.join("second-private-identity.key");
    let first_recipient = generate_identity(&keygen, &first);
    let second_recipient = generate_identity(&keygen, &second);
    let source = "# private source for checksum validation\nprint(17);\n";
    let ciphertext = age.seal(source, &second_recipient).unwrap();
    let combined = temp.0.join("combined-private-identities.key");
    std::fs::write(
        &combined,
        format!(
            "# Native identities only\n{}\n{}\n",
            std::fs::read_to_string(&first).unwrap(),
            std::fs::read_to_string(&second).unwrap()
        ),
    )
    .unwrap();
    assert_eq!(age.open(&ciphertext, &combined).unwrap(), source);

    // Change one checksum symbol while preserving the adapter's native syntax.
    // The real age parser must reject it rather than accepting shape as proof.
    let mut recipient = first_recipient.into_bytes();
    let last = recipient.last_mut().unwrap();
    *last = if *last == b'q' { b'p' } else { b'q' };
    let recipient = String::from_utf8(recipient).unwrap();
    let error = age.seal(source, &recipient).unwrap_err();
    assert_eq!(error, SealedError::OperationFailed);
    assert_sanitized(error, &[source, &recipient]);

    let native = std::fs::read_to_string(&first)
        .unwrap()
        .lines()
        .find(|line| line.starts_with("AGE-SECRET-KEY-1"))
        .unwrap()
        .to_owned();
    let mut broken = native.into_bytes();
    let last = broken.last_mut().unwrap();
    *last = if *last == b'Q' { b'P' } else { b'Q' };
    let broken = String::from_utf8(broken).unwrap();
    std::fs::write(&first, &broken).unwrap();
    let error = age.open(&ciphertext, &first).unwrap_err();
    assert_eq!(error, SealedError::OperationFailed);
    assert_sanitized(error, &[source, &broken, "first-private-identity"]);
}

fn assert_cli_failure(output: Output, identity: &Path, forbidden: &[&str]) {
    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let all = format!("{stdout}{stderr}");
    assert!(all.contains("age operation failed"));
    assert!(!all.contains(identity.to_str().unwrap()));
    assert!(!all.contains("AGE-SECRET-KEY-1"));
    for text in forbidden {
        assert!(!all.contains(text));
    }
    if !stdout.is_empty() {
        assert!(stdout.contains("\"ok\":false"));
        assert!(stdout.contains("\"data\":null"));
        assert!(stderr.is_empty());
    }
}

#[test]
#[ignore = "requires real age and age-keygen; run cargo test --test swe18_authentication -- --ignored"]
fn real_wrong_key_cannot_reach_cli_queries_execution_or_plaintext_export() {
    let (executable, keygen) = tools();
    let temp = Temp::new();
    let identity = temp.0.join("correct-private-identity.key");
    let wrong_identity = temp.0.join("wrong-private-identity.key");
    let recipient = generate_identity(&keygen, &identity);
    generate_identity(&keygen, &wrong_identity);
    let source =
        "let confidential_symbol = \"private-program-output\"; print(confidential_symbol);\n";
    let ciphertext = Age::with_executable(&executable)
        .seal(source, &recipient)
        .unwrap();
    let encrypted = temp.0.join("program.lim.age");
    std::fs::write(&encrypted, ciphertext).unwrap();
    let executable = executable.canonicalize().unwrap_or(executable);
    let path = if executable.components().count() > 1 {
        let old = std::env::var_os("PATH").unwrap_or_default();
        std::env::join_paths(
            std::iter::once(executable.parent().unwrap().to_path_buf())
                .chain(std::env::split_paths(&old)),
        )
        .unwrap()
    } else {
        std::env::var_os("PATH").unwrap_or_default()
    };
    let make = |command: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lim"));
        child
            .env("PATH", &path)
            .arg(command)
            .arg(&encrypted)
            .arg("--identity")
            .arg(&wrong_identity);
        child
    };
    let forbidden = ["confidential_symbol", "private-program-output"];
    for command in [
        "check",
        "index",
        "symbols",
        "definition",
        "references",
        "complete",
    ] {
        let mut child = make(command);
        child.arg("--json");
        if matches!(command, "definition" | "references" | "complete") {
            child.args(["--line", "1", "--col", "5"]);
        }
        assert_cli_failure(child.output().unwrap(), &wrong_identity, &forbidden);
    }
    for engine in ["tree", "vm"] {
        for json in [false, true] {
            let mut child = make("run");
            child.args(["--engine", engine]);
            if json {
                child.arg("--json");
            }
            assert_cli_failure(child.output().unwrap(), &wrong_identity, &forbidden);
        }
    }
    let plain = temp.0.join("must-not-be-created.lim");
    let output = make("unseal")
        .arg("--out")
        .arg(&plain)
        .arg("--json")
        .output()
        .unwrap();
    assert_cli_failure(output, &wrong_identity, &forbidden);
    assert!(!plain.exists());
}
