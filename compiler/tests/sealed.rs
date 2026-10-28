use limits::sealed::{Age, SealedError, MAX_CIPHERTEXT_BYTES};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        for _ in 0..100 {
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("limits-age-{}-{id}", std::process::id()));
            let builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            let mut builder = builder;
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&p) {
                Ok(()) => return Self(p),
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
fn recipient_shape() -> String {
    format!("age1{}", "q".repeat(58))
}
fn unavailable() -> Age {
    Age::with_executable(std::env::temp_dir().join("limits-age-not-installed-tool"))
}
#[test]
fn missing_external_tool_is_actionable_and_sanitized() {
    let e = unavailable()
        .seal("private-source-secret", &recipient_shape())
        .unwrap_err();
    assert_eq!(e, SealedError::ToolUnavailable);
    assert!(e.to_string().contains("install age"));
    assert!(!e.to_string().contains("private-source"));
}
#[test]
fn recipient_option_injection_is_rejected_before_spawning() {
    for bad in [
        "--recipient",
        "age1;touch /tmp/x",
        "ssh-rsa AAAA",
        "AGE1INVALID",
        "age1\n--identity secret",
    ] {
        assert_eq!(
            unavailable().seal("print(1);", bad),
            Err(SealedError::InvalidRecipient)
        );
    }
}
#[test]
fn source_and_ciphertext_caps_apply_before_tool_use() {
    let large = " ".repeat(limits::lexer::MAX_SOURCE_BYTES + 1);
    assert_eq!(
        unavailable().seal(&large, &recipient_shape()),
        Err(SealedError::InputTooLarge)
    );
    let bytes = vec![0; MAX_CIPHERTEXT_BYTES + 1];
    assert_eq!(
        unavailable().open(&bytes, std::path::Path::new("missing")),
        Err(SealedError::InputTooLarge)
    );
}
#[test]
fn plaintext_and_plugin_identities_are_rejected() {
    assert_eq!(
        unavailable().open(b"print(1);", std::path::Path::new("secret")),
        Err(SealedError::InvalidCiphertext)
    );
    let temp = Temp::new();
    let path = temp.0.join("key.txt");
    std::fs::write(&path, "AGE-PLUGIN-YUBIKEY-unsupported\n").unwrap();
    assert_eq!(
        unavailable().open(b"age-encryption.org/v1\n", &path),
        Err(SealedError::InvalidIdentity)
    );
}
#[test]
fn zero_deadline_cannot_start_a_subprocess() {
    assert_eq!(
        unavailable()
            .with_timeout(Duration::ZERO)
            .seal("", &recipient_shape()),
        Err(SealedError::Timeout)
    );
}

/// Explicitly opt in to the external-tool integration test. It fails if the
/// maintained age tools are missing, rather than silently passing a skipped
/// cryptographic test. Run with LIMITS_AGE_BINARY and LIMITS_AGE_KEYGEN if needed.
#[test]
#[ignore = "requires real age and age-keygen; run cargo test --test sealed -- --ignored"]
fn real_authenticated_roundtrip_tamper_and_wrong_key() {
    let exe = std::env::var_os("LIMITS_AGE_BINARY").unwrap_or_else(|| "age".into());
    let keygen = std::env::var_os("LIMITS_AGE_KEYGEN").unwrap_or_else(|| "age-keygen".into());
    let age = Age::with_executable(exe);
    let temp = Temp::new();
    let mut identities = Vec::new();
    let mut recipients = Vec::new();
    for i in 0..2 {
        let path = temp.0.join(format!("identity-{i}.txt"));
        let result = Command::new(&keygen)
            .arg("-o")
            .arg(&path)
            .output()
            .expect("age-keygen must be installed for this explicit integration test");
        assert!(result.status.success(), "age-keygen failed");
        let public = Command::new(&keygen).arg("-y").arg(&path).output().unwrap();
        assert!(public.status.success());
        recipients.push(String::from_utf8(public.stdout).unwrap().trim().to_owned());
        identities.push(path);
    }
    for source in ["", "# Agent confidential source\nprint(\"π and 😀\");\n"] {
        let a = age.seal(source, &recipients[0]).unwrap();
        let b = age.seal(source, &recipients[0]).unwrap();
        assert_ne!(a, b, "encryption must be randomized");
        assert_eq!(age.open(&a, &identities[0]).unwrap(), source);
        assert_eq!(
            age.open(&a, &identities[1]),
            Err(SealedError::OperationFailed)
        );
        let mut tampered = a.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert_eq!(
            age.open(&tampered, &identities[0]),
            Err(SealedError::OperationFailed)
        );
        assert_eq!(
            age.open(&a[..a.len() - 1], &identities[0]),
            Err(SealedError::OperationFailed)
        );
        if !source.is_empty() {
            assert!(!a.windows(source.len()).any(|w| w == source.as_bytes()));
        }
    }
}

#[cfg(unix)]
#[test]
fn fifo_identity_is_rejected_before_open_can_block() {
    let temp = Temp::new();
    let fifo = temp.0.join("identity.fifo");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    assert_eq!(
        unavailable()
            .with_timeout(Duration::from_millis(20))
            .open(b"age-encryption.org/v1\n", &fifo),
        Err(SealedError::InvalidIdentity)
    );
}

#[test]
#[ignore = "requires real age and age-keygen; run cargo test --test sealed -- --ignored"]
fn real_cli_seal_analyze_execute_unseal_and_no_overwrite() {
    use std::io::Write;
    use std::process::Stdio;
    let age = std::env::var_os("LIMITS_AGE_BINARY").unwrap_or_else(|| "age".into());
    let keygen = std::env::var_os("LIMITS_AGE_KEYGEN").unwrap_or_else(|| "age-keygen".into());
    let temp = Temp::new();
    let identity = temp.0.join("identity.txt");
    let result = Command::new(&keygen)
        .arg("-o")
        .arg(&identity)
        .output()
        .expect("real age-keygen required");
    assert!(result.status.success());
    let result = Command::new(&keygen)
        .arg("-y")
        .arg(&identity)
        .output()
        .unwrap();
    assert!(result.status.success());
    let recipient = String::from_utf8(result.stdout).unwrap().trim().to_string();
    let executable = std::path::Path::new(&age);
    let path = if executable.components().count() > 1 {
        let parent = executable
            .canonicalize()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let old = std::env::var_os("PATH").unwrap_or_default();
        std::env::join_paths(std::iter::once(parent).chain(std::env::split_paths(&old))).unwrap()
    } else {
        std::env::var_os("PATH").unwrap_or_default()
    };
    let make = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lim"));
        command.env("PATH", &path);
        command
    };
    let ciphertext = temp.0.join("program.lim.age");
    let source="# Private source: never a temporary plaintext file.\nlet message=\"agent ready\";print(message);\n";
    let mut child = make()
        .arg("seal")
        .arg("-")
        .arg("--recipient")
        .arg(&recipient)
        .arg("--out")
        .arg(&ciphertext)
        .arg("--json")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "seal failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8(result.stdout)
        .unwrap()
        .contains("\"ok\":true"));
    let bytes = std::fs::read(&ciphertext).unwrap();
    assert!(bytes.starts_with(b"age-encryption.org/v1\n"));
    assert!(!bytes.windows(source.len()).any(|w| w == source.as_bytes()));
    for engine in ["tree", "vm"] {
        let result = make()
            .arg("run")
            .arg(&ciphertext)
            .arg("--identity")
            .arg(&identity)
            .args(["--engine", engine])
            .output()
            .unwrap();
        assert!(result.status.success(), "encrypted run failed");
        assert_eq!(result.stdout, b"agent ready\n");
    }
    for command in ["check", "index"] {
        let result = make()
            .arg(command)
            .arg(&ciphertext)
            .arg("--identity")
            .arg(&identity)
            .arg("--json")
            .output()
            .unwrap();
        assert!(result.status.success(), "encrypted {command} failed");
        let json = String::from_utf8(result.stdout).unwrap();
        assert!(json.contains("\"ok\":true"));
        if command == "index" {
            assert!(json.contains("message"));
        }
        assert!(!json.contains(&identity.to_string_lossy().to_string()));
    }
    // Only the key and ciphertext exist after direct encrypted execution and
    // analysis. The caller did not request a plaintext output file yet.
    assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 2);
    let plain = temp.0.join("restored.lim");
    let result = make()
        .arg("unseal")
        .arg(&ciphertext)
        .arg("--identity")
        .arg(&identity)
        .arg("--out")
        .arg(&plain)
        .arg("--json")
        .output()
        .unwrap();
    assert!(result.status.success());
    let json = String::from_utf8(result.stdout).unwrap();
    assert!(json.contains("utf-8-limits"));
    assert_eq!(std::fs::read_to_string(&plain).unwrap(), source);
    let result = make()
        .arg("unseal")
        .arg(&ciphertext)
        .arg("--identity")
        .arg(&identity)
        .arg("--out")
        .arg(&plain)
        .arg("--json")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(std::fs::read_to_string(&plain).unwrap(), source);
    let result = make()
        .arg("seal")
        .arg(&plain)
        .arg("--recipient")
        .arg(&recipient)
        .arg("--out")
        .arg(&ciphertext)
        .arg("--json")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(std::fs::read(&ciphertext).unwrap(), bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&plain).unwrap().permissions().mode() & 0o077,
            0
        );
        assert_eq!(
            std::fs::metadata(&ciphertext).unwrap().permissions().mode() & 0o077,
            0
        );
    }
}
