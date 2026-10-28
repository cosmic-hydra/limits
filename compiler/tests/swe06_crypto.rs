use limits::sealed::{Age, SealedError};
use std::path::PathBuf;
use std::process::Command;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        for _ in 0..100 {
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("limits-swe06-{}-{id}", std::process::id()));
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

#[test]
fn complete_identity_is_validated_before_the_tool_can_run() {
    let temp = Temp::new();
    let identity = temp.0.join("private-identity-path-must-not-escape");
    let age = Age::with_executable(temp.0.join("missing-tool"));
    // This is a native key's syntax, not a valid key checksum. Checksum parsing
    // belongs to the trusted age executable and must never enable plugins.
    let native_shape = format!("AGE-SECRET-KEY-1{}", "Q".repeat(58));
    let oversized = format!("#{}\n{native_shape}\n", "x".repeat(65_536));
    let mixed = format!("{native_shape}\nAGE-PLUGIN-YUBIKEY-unsupported\n");
    for contents in [
        b"# only comments\n\n".as_slice(),
        b"\xff\xfe".as_slice(),
        oversized.as_bytes(),
        mixed.as_bytes(),
    ] {
        std::fs::write(&identity, contents).unwrap();
        let error = age.open(b"age-encryption.org/v1\n", &identity).unwrap_err();
        assert_eq!(error, SealedError::InvalidIdentity);
        assert!(!error.to_string().contains("private-identity-path"));
        assert!(!format!("{error:?}").contains("AGE-SECRET"));
    }
    std::fs::remove_file(&identity).unwrap();
    std::fs::create_dir(&identity).unwrap();
    assert_eq!(
        age.open(b"age-encryption.org/v1\n", &identity),
        Err(SealedError::InvalidIdentity)
    );
}

#[test]
#[ignore = "requires real age and age-keygen; run cargo test --test swe06_crypto -- --ignored"]
fn real_streamed_plaintext_is_withheld_until_the_final_authentication_succeeds() {
    use std::io::Write;
    use std::process::Stdio;

    let executable = std::env::var_os("LIMITS_AGE_BINARY").unwrap_or_else(|| "age".into());
    let keygen = std::env::var_os("LIMITS_AGE_KEYGEN").unwrap_or_else(|| "age-keygen".into());
    let temp = Temp::new();
    let identity = temp.0.join("identity.txt");
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
    let age = Age::with_executable(&executable);

    // More than two age payload chunks ensures a streaming decryptor releases
    // authenticated earlier chunks before it discovers a damaged final chunk.
    let source = "# private streamed source\n".repeat(10_000);
    let mut ciphertext = age.seal(&source, &recipient).unwrap();
    assert_eq!(age.open(&ciphertext, &identity).unwrap(), source);
    let last = ciphertext.len() - 1;
    ciphertext[last] ^= 1;
    let damaged = temp.0.join("damaged.lim.age");
    std::fs::write(&damaged, &ciphertext).unwrap();
    let direct = Command::new(&executable)
        .arg("--decrypt")
        .arg("--identity")
        .arg(&identity)
        .arg("--")
        .arg(&damaged)
        .output()
        .unwrap();
    assert!(!direct.status.success());
    assert!(
        !direct.stdout.is_empty(),
        "the fixture must exercise a real age failure after partial plaintext"
    );
    assert_eq!(
        age.open(&ciphertext, &identity),
        Err(SealedError::OperationFailed)
    );

    // A successfully authenticated byte stream must still pass the source's
    // UTF-8 boundary before the adapter can expose a String.
    let mut child = Command::new(&executable)
        .args(["--encrypt", "--recipient", &recipient, "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"\xff\xfe").unwrap();
    let binary = child.wait_with_output().unwrap();
    assert!(binary.status.success());
    assert_eq!(
        age.open(&binary.stdout, &identity),
        Err(SealedError::InvalidSource)
    );
}
