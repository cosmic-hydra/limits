//! Invocation-scoped builtin capabilities, output budgets and cooperative stop
//! controls. These controls do not replace an OS process/memory sandbox and do
//! not interrupt a blocking host writer or a single long host operation.
use crate::error::{Kind, LimError, Span};
use crate::interp::{Interpreter, Limits, BUILTINS};
use crate::{codegen, lexer, parser, vm::Vm, Engine};
use std::collections::BTreeSet;
use std::io::{self, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

/// May be cloned into a supervisor thread while the engine stays on its worker.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);
impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Capabilities attach to a running invocation, never to source names: renaming
/// `print`, storing it in a list, or calling it through a closure cannot grant it.
#[derive(Debug, Clone, Default)]
pub struct SandboxPolicy {
    /// None permits the existing builtin set; Some(empty) denies every builtin.
    pub allowed_builtins: Option<BTreeSet<String>>,
    /// Aggregate UTF-8 output bytes, including spaces and newline bytes.
    pub max_output_bytes: Option<u64>,
    /// Execution time checked at statement/call/iteration boundaries.
    pub timeout: Option<Duration>,
    pub cancellation: Option<CancellationToken>,
}
impl SandboxPolicy {
    /// Practical bounded-output/deadline defaults; all existing builtins allowed.
    pub fn restricted() -> Self {
        Self {
            max_output_bytes: Some(1024 * 1024),
            timeout: Some(Duration::from_secs(5)),
            ..Self::default()
        }
    }
    /// Only builtins with no output or container mutation. Assignment in the
    /// language remains available: this is a capability profile, not purity typing.
    pub fn pure() -> Self {
        Self {
            allowed_builtins: Some(
                BUILTINS
                    .iter()
                    .copied()
                    .filter(|b| !matches!(*b, "print" | "push" | "pop"))
                    .map(str::to_owned)
                    .collect(),
            ),
            ..Self::restricted()
        }
    }
    pub fn allow_only(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allowed_builtins: Some(names.into_iter().map(Into::into).collect()),
            ..Self::default()
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if let Some(allowed) = &self.allowed_builtins {
            for name in allowed {
                if !BUILTINS.contains(&name.as_str()) {
                    return Err(format!("unknown builtin in sandbox policy: '{name}'"));
                }
            }
        }
        Ok(())
    }
}

/// Run an isolated invocation with explicit capabilities in either engine.
/// Like `run_with_engine`, this leaves optional static checks to the caller.
pub fn run(
    src: &str,
    out: &mut dyn Write,
    limits: Limits,
    engine: Engine,
    policy: SandboxPolicy,
) -> Result<(), LimError> {
    let program = parser::parse(lexer::lex(src)?)?;
    match engine {
        Engine::Tree => Interpreter::with_policy(out, limits, policy).run(&program),
        Engine::Vm => Vm::with_policy(out, limits, policy).run(codegen::compile(&program)?),
    }
}
fn error(msg: impl Into<String>, span: Span) -> LimError {
    LimError::new(Kind::Runtime, msg, span)
}

pub(crate) struct State {
    policy: SandboxPolicy,
    started: Option<Instant>,
    output_bytes: u64,
}
impl State {
    pub(crate) fn new(policy: SandboxPolicy) -> Self {
        Self {
            policy,
            started: None,
            output_bytes: 0,
        }
    }
    /// Tree interpreters retain cumulative output budgets, as they retain their
    /// statement budget. A direct run gets a fresh cooperative deadline.
    pub(crate) fn start(&mut self) -> Result<(), LimError> {
        self.policy
            .validate()
            .map_err(|m| error(m, Span { line: 1, col: 1 }))?;
        self.started = Some(Instant::now());
        Ok(())
    }
    /// VM invocations are isolated, including successful/error reuse.
    pub(crate) fn reset(&mut self) -> Result<(), LimError> {
        self.output_bytes = 0;
        self.start()
    }
    #[inline]
    pub(crate) fn checkpoint(&self, span: Span) -> Result<(), LimError> {
        if self
            .policy
            .cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(error("execution cancelled", span));
        }
        if let (Some(limit), Some(started)) = (self.policy.timeout, self.started) {
            if started.elapsed() >= limit {
                return Err(error("execution deadline exceeded", span));
            }
        }
        Ok(())
    }
    pub(crate) fn check_builtin(&self, name: &str, span: Span) -> Result<(), LimError> {
        if self
            .policy
            .allowed_builtins
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(name))
        {
            return Err(error(format!("sandbox denied builtin '{name}'"), span));
        }
        Ok(())
    }
    pub(crate) fn write(
        &mut self,
        out: &mut dyn Write,
        bytes: &[u8],
        span: Span,
    ) -> Result<(), LimError> {
        if let Some(max) = self.policy.max_output_bytes {
            let requested = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            if requested > max.saturating_sub(self.output_bytes) {
                return Err(error(format!("output limit exceeded ({max} bytes)"), span));
            }
        }
        let mut remaining = bytes;
        while !remaining.is_empty() {
            self.checkpoint(span)?;
            match out.write(remaining) {
                Ok(0) => {
                    return Err(error(
                        io::Error::from(io::ErrorKind::WriteZero).to_string(),
                        span,
                    ))
                }
                Ok(n) if n <= remaining.len() => {
                    self.output_bytes = self.output_bytes.saturating_add(n as u64);
                    remaining = &remaining[n..];
                }
                Ok(_) => return Err(error("host writer returned an invalid byte count", span)),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(error(e.to_string(), span)),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Partial {
        first: bool,
    }
    impl Write for Partial {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.first {
                self.first = false;
                Ok(bytes.len().min(1))
            } else {
                Err(io::Error::other("sink failed"))
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn counts_only_bytes_successfully_accepted_by_writer() {
        let mut state = State::new(SandboxPolicy {
            max_output_bytes: Some(3),
            ..SandboxPolicy::default()
        });
        state.start().unwrap();
        let span = Span { line: 3, col: 4 };
        assert_eq!(
            state
                .write(&mut Partial { first: true }, b"abc", span)
                .unwrap_err()
                .msg,
            "sink failed"
        );
        assert_eq!(state.output_bytes, 1);
        let mut out = Vec::new();
        state.write(&mut out, b"xy", span).unwrap();
        assert_eq!(
            state.write(&mut out, b"z", span).unwrap_err().msg,
            "output limit exceeded (3 bytes)"
        );
        assert_eq!(out, b"xy");
    }
}
