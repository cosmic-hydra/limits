//! Filesystem boundary regressions. The Unix age double exercises bounded CLI
//! plumbing only; the explicitly ignored test uses the maintained age tools.
#![cfg(unix)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        for _ in 0..100 {
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("limits-swe17-{}-{id}", std::process::id()));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
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

fn invoke(mut command: Command, source: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Err(e) = child.stdin.take().unwrap().write_all(source) {
        assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe);
    }
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

fn assert_no_private_details(json: &str, identity: &Path, output: &Path) {
    for private in [
        identity.to_str().unwrap(),
        output.to_str().unwrap(),
        "AGE-SECRET-KEY-1",
        "private-source-marker",
        "private-child-stderr",
    ] {
        assert!(!json.contains(private), "{json}");
    }
}

#[cfg(unix)]
struct Plumbing {
    temp: Temp,
    identity: PathBuf,
    source: PathBuf,
    ciphertext: PathBuf,
    recipient: String,
}
#[cfg(unix)]
impl Plumbing {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let temp = Temp::new();
        let executable = temp.0.join("age");
        // Deliberately NOT cryptography. Consume stdin fully to test the real
        // CLI's pipe handling, byte caps, UTF-8 checks and output creation.
        std::fs::write(
            &executable,
            b"#!/bin/sh\ncase \"$1\" in\n--encrypt) printf 'age-encryption.org/v1\\n'; /bin/cat ;;\n--decrypt) IFS= read -r header; /bin/cat ;;\n*) printf 'private-child-stderr' >&2; exit 7 ;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let identity = temp.0.join("private-identity.key");
        std::fs::write(&identity, format!("AGE-SECRET-KEY-1{}\n", "Q".repeat(58))).unwrap();
        std::fs::set_permissions(&identity, std::fs::Permissions::from_mode(0o600)).unwrap();
        let source = temp.0.join("source.lim");
        std::fs::write(&source, "# private-source-marker\nprint(\"unicode é\");\n").unwrap();
        let ciphertext = temp.0.join("source.lim.age");
        let mut bytes = b"age-encryption.org/v1\n".to_vec();
        bytes.extend(std::fs::read(&source).unwrap());
        std::fs::write(&ciphertext, bytes).unwrap();
        Self {
            temp,
            identity,
            source,
            ciphertext,
            recipient: format!("age1{}", "q".repeat(58)),
        }
    }

    fn append_args(&self, command: &mut Command, operation: &str, source: &Path, output: &Path) {
        command.arg(operation).arg(source);
        if operation == "seal" {
            command.args(["--recipient", &self.recipient]);
        } else {
            command.arg("--identity").arg(&self.identity);
        }
        command.arg("--out").arg(output).arg("--json");
        command.env("PATH", &self.temp.0);
    }

    fn command(&self, operation: &str, source: &Path, output: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lim"));
        self.append_args(&mut command, operation, source, output);
        command
    }
}

#[cfg(unix)]
#[test]
fn source_identity_and_alias_collisions_preserve_every_existing_file() {
    use std::os::unix::fs::symlink;
    let p = Plumbing::new();
    let initial: Vec<_> = [&p.identity, &p.source, &p.ciphertext]
        .into_iter()
        .map(|path| (path.clone(), std::fs::read(path).unwrap()))
        .collect();
    let hardlink = p.temp.0.join("identity-hardlink");
    std::fs::hard_link(&p.identity, &hardlink).unwrap();
    let symlink_path = p.temp.0.join("source-symlink");
    symlink(&p.source, &symlink_path).unwrap();
    let dangling = p.temp.0.join("dangling");
    let missing = p.temp.0.join("must-stay-absent");
    symlink(&missing, &dangling).unwrap();
    let directory = p.temp.0.join("directory");
    std::fs::create_dir(&directory).unwrap();

    for (operation, source) in [("seal", &p.source), ("unseal", &p.ciphertext)] {
        for output in [
            &p.source,
            &p.ciphertext,
            &p.identity,
            &hardlink,
            &symlink_path,
            &dangling,
            &directory,
        ] {
            let result = invoke(p.command(operation, source, output), b"");
            let json = response(&result, 2);
            assert!(
                json.contains("existing files are never overwritten"),
                "{json}"
            );
            assert!(json.contains("\"code\":\"LIM_HOST\""), "{json}");
            // Source paths are deliberately part of the response envelope;
            // identity contents and diagnostic host paths must not be included.
            assert!(!json.contains(p.identity.to_str().unwrap()), "{json}");
            assert!(!json.contains("AGE-SECRET-KEY-1"), "{json}");
            for (path, bytes) in &initial {
                assert_eq!(&std::fs::read(path).unwrap(), bytes);
            }
            assert!(!missing.exists());
            assert!(std::fs::symlink_metadata(&dangling)
                .unwrap()
                .file_type()
                .is_symlink());
        }
    }
}

#[cfg(unix)]
#[test]
fn permissive_umask_and_symlinked_source_keep_outputs_private() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let p = Plumbing::new();
    let alias = p.temp.0.join("alias.lim");
    symlink(&p.source, &alias).unwrap();
    for (operation, source, name) in [
        ("seal", &alias, "new.lim.age"),
        ("unseal", &p.ciphertext, "new.lim"),
    ] {
        let output = p.temp.0.join(name);
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "umask 000; exec \"$@\"", "limits-swe17-umask"])
            .arg(env!("CARGO_BIN_EXE_lim"));
        p.append_args(&mut command, operation, source, &output);
        let result = invoke(command, b"");
        assert!(response(&result, 0).contains("\"ok\":true"));
        assert_eq!(
            std::fs::metadata(output).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        std::fs::read(&alias).unwrap(),
        std::fs::read(&p.source).unwrap()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn partial_write_errors_remove_both_ciphertext_and_requested_plaintext() {
    let p = Plumbing::new();
    let source = vec![b' '; 8192];
    std::fs::write(&p.source, &source).unwrap();
    let mut ciphertext = b"age-encryption.org/v1\n".to_vec();
    ciphertext.extend(&source);
    std::fs::write(&p.ciphertext, ciphertext).unwrap();
    let before = std::fs::read_dir(&p.temp.0).unwrap().count();
    for (operation, source, name) in [
        ("seal", &p.source, "partial.lim.age"),
        ("unseal", &p.ciphertext, "partial.lim"),
    ] {
        let output = p.temp.0.join(name);
        let mut command = Command::new("/bin/sh");
        // Ignoring SIGXFSZ makes the kernel return EFBIG after the first
        // limited bytes, exercising write_new's actual partial-write cleanup.
        command
            .args([
                "-c",
                "trap '' XFSZ; ulimit -f 1; exec \"$@\"",
                "limits-swe17-write",
            ])
            .arg(env!("CARGO_BIN_EXE_lim"));
        p.append_args(&mut command, operation, source, &output);
        let result = invoke(command, b"");
        let json = response(&result, 2);
        assert!(json.contains("cannot write output"), "{json}");
        assert_no_private_details(json, &p.identity, &output);
        assert!(!output.exists());
        assert_eq!(std::fs::read_dir(&p.temp.0).unwrap().count(), before);
    }
}

#[cfg(unix)]
#[test]
fn source_and_decrypted_byte_caps_are_exact_and_fail_without_outputs() {
    let p = Plumbing::new();
    let output = p.temp.0.join("bounded.lim.age");
    let cap = limits::lexer::MAX_SOURCE_BYTES;
    std::fs::write(&p.source, vec![b' '; cap]).unwrap();
    let result = invoke(p.command("seal", &p.source, &output), b"");
    assert!(response(&result, 0).contains(&format!("\"bytes\":{}", cap + 22)));
    let restored = p.temp.0.join("bounded.lim");
    let result = invoke(p.command("unseal", &output, &restored), b"");
    assert!(response(&result, 0).contains(&format!("\"bytes\":{cap}")));
    assert_eq!(std::fs::metadata(&restored).unwrap().len(), cap as u64);

    std::fs::remove_file(&output).unwrap();
    std::fs::remove_file(&restored).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&p.source)
        .unwrap()
        .write_all("é".as_bytes())
        .unwrap();
    let result = invoke(p.command("seal", &p.source, &output), b"");
    let json = response(&result, 1);
    assert!(json.contains("source size limit exceeded"), "{json}");
    assert!(!json.contains("not valid UTF-8"), "{json}");
    assert!(!output.exists());

    let mut oversized = b"age-encryption.org/v1\n".to_vec();
    oversized.resize(22 + cap + 1, b' ');
    std::fs::write(&p.ciphertext, oversized).unwrap();
    let result = invoke(p.command("unseal", &p.ciphertext, &restored), b"");
    let json = response(&result, 2);
    assert!(
        json.contains("encrypted program output exceeds the size limit"),
        "{json}"
    );
    assert_no_private_details(json, &p.identity, &restored);
    assert!(!restored.exists());
}

#[cfg(unix)]
#[test]
fn ciphertext_cap_utf8_and_missing_paths_reject_without_creating_outputs() {
    let p = Plumbing::new();
    let output = p.temp.0.join("must-stay-absent.lim");
    let cap = limits::sealed::MAX_CIPHERTEXT_BYTES;
    let mut ciphertext = b"age-encryption.org/v1\n".to_vec();
    ciphertext.resize(cap, b' ');
    std::fs::write(&p.ciphertext, &ciphertext).unwrap();
    let result = invoke(p.command("unseal", &p.ciphertext, &output), b"");
    let json = response(&result, 2);
    // Exactly the ciphertext cap gets as far as the independent plaintext cap.
    assert!(
        json.contains("encrypted program output exceeds the size limit"),
        "{json}"
    );
    ciphertext.push(0xff);
    std::fs::write(&p.ciphertext, ciphertext).unwrap();
    let result = invoke(p.command("unseal", &p.ciphertext, &output), b"");
    let json = response(&result, 2);
    assert!(json.contains("source size limit exceeded"), "{json}");
    assert_no_private_details(json, &p.identity, &output);
    assert!(!output.exists());

    {
        let bytes = b"private-source-marker\xff";
        std::fs::write(&p.source, bytes).unwrap();
        let sealed = p.temp.0.join("invalid.lim.age");
        let result = invoke(p.command("seal", &p.source, &sealed), b"");
        let json = response(&result, 2);
        assert!(json.contains("source is not valid UTF-8"), "{json}");
        assert_no_private_details(json, &p.identity, &sealed);
        assert!(!sealed.exists());

        let mut ciphertext = b"age-encryption.org/v1\n".to_vec();
        ciphertext.extend(bytes);
        std::fs::write(&p.ciphertext, ciphertext).unwrap();
        let result = invoke(p.command("unseal", &p.ciphertext, &output), b"");
        let json = response(&result, 2);
        assert!(
            json.contains("decrypted program is not valid UTF-8"),
            "{json}"
        );
        assert_no_private_details(json, &p.identity, &output);
        assert!(!output.exists());
    }

    std::fs::write(&p.source, b"").unwrap();
    let bad_destination = p.temp.0.join("missing-parent/private-output.lim.age");
    let result = invoke(p.command("seal", &p.source, &bad_destination), b"");
    let json = response(&result, 2);
    assert!(json.contains("cannot create output"), "{json}");
    assert_no_private_details(json, &p.identity, &bad_destination);
    let missing_source = p.temp.0.join("private-missing-source.lim");
    let result = invoke(p.command("seal", &missing_source, &bad_destination), b"");
    let json = response(&result, 2);
    assert!(json.contains("cannot open source file"), "{json}");
    assert_no_private_details(json, &p.identity, &bad_destination);
    assert!(!bad_destination.exists());
}

#[test]
fn exact_identity_cap_and_stable_regular_symlink_are_accepted() {
    use limits::sealed::{Age, SealedError};
    use std::os::unix::fs::symlink;
    let p = Plumbing::new();
    let native = format!("AGE-SECRET-KEY-1{}\n", "Q".repeat(58));
    let mut identity = vec![b'#'];
    identity.resize(65_536 - native.len() - 1, b'x');
    identity.push(b'\n');
    identity.extend(native.as_bytes());
    assert_eq!(identity.len(), 65_536);
    std::fs::write(&p.identity, &identity).unwrap();
    let alias = p.temp.0.join("identity-alias");
    symlink(&p.identity, &alias).unwrap();
    let age = Age::with_executable(p.temp.0.join("missing-trusted-age"));
    // The host keeps this private directory and target unchanged. A missing
    // executable proves complete identity validation accepted the exact cap.
    assert_eq!(
        age.open(b"age-encryption.org/v1\n", &alias),
        Err(SealedError::ToolUnavailable)
    );
    identity.push(b'\n');
    std::fs::write(&p.identity, identity).unwrap();
    assert_eq!(
        age.open(b"age-encryption.org/v1\n", &alias),
        Err(SealedError::InvalidIdentity)
    );
}

#[test]
#[ignore = "requires real age and age-keygen; run cargo test --test swe17_filesystem -- --ignored"]
fn real_age_maximum_source_roundtrip_leaves_only_requested_outputs() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let executable = std::env::var_os("LIMITS_AGE_BINARY").unwrap_or_else(|| "age".into());
    let keygen = std::env::var_os("LIMITS_AGE_KEYGEN").unwrap_or_else(|| "age-keygen".into());
    let temp = Temp::new();
    let identity = temp.0.join("protected-identity.key");
    let generated = Command::new(&keygen)
        .arg("-o")
        .arg(&identity)
        .output()
        .expect("real age-keygen must be installed for this explicit test");
    assert!(generated.status.success(), "age-keygen failed");
    let public = Command::new(&keygen)
        .arg("-y")
        .arg(&identity)
        .output()
        .unwrap();
    assert!(public.status.success());
    let recipient = String::from_utf8(public.stdout).unwrap().trim().to_owned();
    let identity_alias = temp.0.join("protected-identity-alias.key");
    symlink(&identity, &identity_alias).unwrap();
    let path = if Path::new(&executable).components().count() > 1 {
        let full = Path::new(&executable).canonicalize().unwrap();
        std::env::join_paths(std::iter::once(full.parent().unwrap().to_path_buf()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap()
    } else {
        std::env::var_os("PATH").unwrap_or_default()
    };
    let scratch = temp.0.join("empty-tmpdir");
    std::fs::create_dir(&scratch).unwrap();
    let make = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lim"));
        command.env("PATH", &path).env("TMPDIR", &scratch);
        command
    };
    let mut source = b"# private-source-marker: unicode ".to_vec();
    source.extend("é 😀\n".as_bytes());
    source.resize(limits::lexer::MAX_SOURCE_BYTES, b' ');
    let ciphertext = temp.0.join("maximum.lim.age");
    let mut command = make();
    command
        .args(["seal", "-", "--recipient", &recipient, "--out"])
        .arg(&ciphertext)
        .arg("--json");
    let result = invoke(command, &source);
    assert!(response(&result, 0).contains("\"format\":\"age-v1\""));
    assert!(
        std::fs::metadata(&ciphertext).unwrap().len()
            <= limits::sealed::MAX_CIPHERTEXT_BYTES as u64
    );

    let mut command = make();
    command
        .arg("check")
        .arg(&ciphertext)
        .arg("--identity")
        .arg(&identity_alias)
        .arg("--json");
    let result = invoke(command, b"");
    assert!(response(&result, 0).contains("\"ok\":true"));
    // Key, stable key alias, ciphertext and the empty configured temporary
    // directory are the only entries before explicit plaintext export.
    assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 4);
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);

    let output = temp.0.join("requested-maximum.lim");
    let mut command = make();
    command
        .arg("unseal")
        .arg(&ciphertext)
        .arg("--identity")
        .arg(&identity_alias)
        .arg("--out")
        .arg(&output)
        .arg("--json");
    let result = invoke(command, b"");
    assert!(response(&result, 0).contains("\"format\":\"utf-8-limits\""));
    assert_eq!(std::fs::read(&output).unwrap(), source);
    assert_eq!(
        std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 5);
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
}
