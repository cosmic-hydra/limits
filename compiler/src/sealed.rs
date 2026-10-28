//! Optional authenticated encryption through the maintained `age` executable.
//!
//! Keys authorize their holders; encryption cannot distinguish an AI agent from
//! a human with the same key. This module deliberately does not implement crypto,
//! write plaintext to disk, expose child diagnostics, or launch a shell. Use a
//! trusted executable and a protected native X25519 identity file.
use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const MAX_CIPHERTEXT_BYTES: usize = crate::lexer::MAX_SOURCE_BYTES + 65_536;
const MAX_IDENTITY_BYTES: u64 = 65_536;
const HEADER: &[u8] = b"age-encryption.org/v1\n";

/// Errors contain no source text, identity contents, child stderr, or host paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealedError {
    ToolUnavailable,
    InvalidRecipient,
    InvalidIdentity,
    InvalidCiphertext,
    InputTooLarge,
    OutputTooLarge,
    Timeout,
    OperationFailed,
    InvalidSource,
}
impl fmt::Display for SealedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ToolUnavailable => "age executable is unavailable; install age or configure a trusted executable",
            Self::InvalidRecipient => "expected a native age X25519 recipient key",
            Self::InvalidIdentity => "expected a readable native age X25519 identity file",
            Self::InvalidCiphertext => "expected a binary age-encrypted program",
            Self::InputTooLarge => "encrypted program input exceeds the size limit",
            Self::OutputTooLarge => "encrypted program output exceeds the size limit",
            Self::Timeout => "age operation exceeded its deadline",
            Self::OperationFailed => "age operation failed; decryption requires the matching identity and intact ciphertext",
            Self::InvalidSource => "decrypted program is not valid UTF-8",
        })
    }
}
impl std::error::Error for SealedError {}

/// Host-controlled adapter. `with_executable` must never be supplied by a `.lim`
/// program or an untrusted request. Native recipient/identity keys are required;
/// password prompts, SSH keys, and external age plugins are not supported.
/// Identity files and parent directories must be protected and immutable for
/// the operation: age reopens the canonical identity path after validation.
#[derive(Debug, Clone)]
pub struct Age {
    executable: PathBuf,
    timeout: Duration,
}
impl Default for Age {
    fn default() -> Self {
        Self::with_executable("age")
    }
}
impl Age {
    pub fn with_executable(path: impl Into<PathBuf>) -> Self {
        Self {
            executable: path.into(),
            timeout: Duration::from_secs(10),
        }
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    /// Encrypt UTF-8 source to standard binary age ciphertext, with a fresh
    /// randomized file key on every invocation. No plaintext file is created.
    pub fn seal(&self, source: &str, recipient: &str) -> Result<Vec<u8>, SealedError> {
        if source.len() > crate::lexer::MAX_SOURCE_BYTES {
            return Err(SealedError::InputTooLarge);
        }
        if !native_recipient(recipient) {
            return Err(SealedError::InvalidRecipient);
        }
        let mut command = Command::new(&self.executable);
        command.args(["--encrypt", "--recipient", recipient, "--"]);
        let result = invoke(
            command,
            source.as_bytes().to_vec(),
            MAX_CIPHERTEXT_BYTES,
            self.timeout,
        )?;
        if !result.starts_with(HEADER) {
            return Err(SealedError::OperationFailed);
        }
        Ok(result)
    }
    /// Authenticate and decrypt entirely in memory. Child output is returned
    /// only after successful process completion, so unauthenticated partial
    /// plaintext emitted by a streaming decryptor is never exposed.
    pub fn open(&self, ciphertext: &[u8], identity: &Path) -> Result<String, SealedError> {
        if ciphertext.len() > MAX_CIPHERTEXT_BYTES {
            return Err(SealedError::InputTooLarge);
        }
        if !ciphertext.starts_with(HEADER) {
            return Err(SealedError::InvalidCiphertext);
        }
        let identity = native_identity(identity)?;
        let mut command = Command::new(&self.executable);
        command
            .arg("--decrypt")
            .arg("--identity")
            .arg(identity)
            .arg("--");
        let source = invoke(
            command,
            ciphertext.to_vec(),
            crate::lexer::MAX_SOURCE_BYTES,
            self.timeout,
        )?;
        String::from_utf8(source).map_err(|_| SealedError::InvalidSource)
    }
}
fn native_recipient(key: &str) -> bool {
    key.len() == 62
        && key.starts_with("age1")
        && key[4..]
            .bytes()
            .all(|b| b"qpzry9x8gf2tvdw0s3jn54khce6mua7l".contains(&b))
}
fn native_identity(path: &Path) -> Result<PathBuf, SealedError> {
    let canonical = path
        .canonicalize()
        .map_err(|_| SealedError::InvalidIdentity)?;
    // Check the file type before opening: opening a FIFO could block before
    // the child-process deadline starts. Identity paths and their parent
    // directories must be host-owned and immutable for the operation because
    // age itself opens this same canonical path after this validation.
    if !canonical
        .metadata()
        .map_err(|_| SealedError::InvalidIdentity)?
        .is_file()
    {
        return Err(SealedError::InvalidIdentity);
    }
    let file = std::fs::File::open(&canonical).map_err(|_| SealedError::InvalidIdentity)?;
    let mut contents = Vec::new();
    file.take(MAX_IDENTITY_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|_| SealedError::InvalidIdentity)?;
    if contents.len() as u64 > MAX_IDENTITY_BYTES {
        return Err(SealedError::InvalidIdentity);
    }
    let text = std::str::from_utf8(&contents).map_err(|_| SealedError::InvalidIdentity)?;
    let mut keys = 0;
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        // Reject plugin/SSH/password identity material before age can invoke a
        // plugin or prompt. Bech32 checksum validation belongs to age itself.
        if line.len() != 74
            || !line.starts_with("AGE-SECRET-KEY-1")
            || !line[16..]
                .bytes()
                .all(|b| b"QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7L".contains(&b))
        {
            return Err(SealedError::InvalidIdentity);
        }
        keys += 1;
    }
    if keys == 0 {
        return Err(SealedError::InvalidIdentity);
    }
    Ok(canonical)
}

enum Event {
    Written(bool),
    Read(Result<Vec<u8>, SealedError>),
}
fn invoke(
    mut command: Command,
    input: Vec<u8>,
    cap: usize,
    timeout: Duration,
) -> Result<Vec<u8>, SealedError> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or(SealedError::Timeout)?;
    if timeout.is_zero() {
        return Err(SealedError::Timeout);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SealedError::ToolUnavailable
            } else {
                SealedError::OperationFailed
            }
        })?;
    let mut stdin = child.stdin.take().ok_or(SealedError::OperationFailed)?;
    let stdout = child.stdout.take().ok_or(SealedError::OperationFailed)?;
    let (send, receive) = mpsc::channel();
    let writer = send.clone();
    if std::thread::Builder::new()
        .name("limits-age-input".into())
        .spawn(move || {
            let ok = stdin.write_all(&input).is_ok();
            drop(stdin);
            let _ = writer.send(Event::Written(ok));
        })
        .is_err()
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(SealedError::OperationFailed);
    }
    if std::thread::Builder::new()
        .name("limits-age-output".into())
        .spawn(move || {
            let mut out = Vec::new();
            let result = if stdout.take(cap as u64 + 1).read_to_end(&mut out).is_err() {
                Err(SealedError::OperationFailed)
            } else if out.len() > cap {
                Err(SealedError::OutputTooLarge)
            } else {
                Ok(out)
            };
            let _ = send.send(Event::Read(result));
        })
        .is_err()
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(SealedError::OperationFailed);
    }
    let result = wait(&mut child, receive, deadline);
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}
fn wait(
    child: &mut std::process::Child,
    events: mpsc::Receiver<Event>,
    deadline: Instant,
) -> Result<Vec<u8>, SealedError> {
    let mut written = false;
    let mut output = None;
    let mut status: Option<ExitStatus> = None;
    loop {
        if Instant::now() >= deadline {
            return Err(SealedError::Timeout);
        }
        while let Ok(event) = events.try_recv() {
            match event {
                Event::Written(false) => return Err(SealedError::OperationFailed),
                Event::Written(true) => written = true,
                Event::Read(result) => output = Some(result?),
            }
        }
        if status.is_none() {
            status = child.try_wait().map_err(|_| SealedError::OperationFailed)?;
        }
        if status.is_some_and(|s| !s.success()) {
            return Err(SealedError::OperationFailed);
        }
        if written && status.is_some_and(|s| s.success()) {
            if let Some(output) = output {
                return Ok(output);
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn helper(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "sealed::tests::process_helper", "--nocapture"])
            .env("LIMITS_SEALED_TEST_HELPER", mode);
        command
    }
    #[test]
    fn process_helper() {
        match std::env::var("LIMITS_SEALED_TEST_HELPER").as_deref() {
            Ok("wait") => std::thread::sleep(Duration::from_secs(2)),
            Ok("overflow") => {
                let _ = std::io::stdout().write_all(&vec![b'x'; 4096]);
                std::thread::sleep(Duration::from_secs(2));
            }
            Ok("fail") => {
                eprintln!("private-key-and-plaintext-must-not-escape");
                std::process::exit(7);
            }
            _ => {}
        }
    }
    #[test]
    fn child_deadline_is_enforced_and_owned_child_reaped() {
        let before = Instant::now();
        assert_eq!(
            invoke(helper("wait"), vec![], 4096, Duration::from_millis(30)),
            Err(SealedError::Timeout)
        );
        assert!(before.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn child_output_is_bounded() {
        assert_eq!(
            invoke(helper("overflow"), vec![], 128, Duration::from_secs(5)),
            Err(SealedError::OutputTooLarge)
        );
    }
    #[test]
    fn child_failure_does_not_expose_stderr_or_output() {
        let err = invoke(
            helper("fail"),
            b"secret input".to_vec(),
            4096,
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert_eq!(err, SealedError::OperationFailed);
        assert!(!err.to_string().contains("private-key"));
        assert!(!err.to_string().contains("secret input"));
    }
}
