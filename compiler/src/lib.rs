//! Limits: a small, explicit language for AI agents.
//! Pipeline: source -> lexer -> parser (AST) -> tree interpreter or bytecode VM.

pub mod ast;
pub mod error;
pub mod interp;
pub mod lexer;
pub mod parser;
pub mod resolve;

use error::LimError;

/// Lex, parse, and run a program. Output from `print` goes to `out`.
pub fn run(src: &str, out: &mut dyn std::io::Write) -> Result<(), LimError> {
    run_with_engine(src, out, interp::Limits::default(), Engine::Tree)
}

pub mod bytecode;
pub mod codegen;
pub mod vm;

pub mod value;

/// Select an execution engine. Tree remains the compatibility default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Engine {
    #[default]
    Tree,
    Vm,
}

/// Execute an isolated invocation with explicit engine and execution limits.
/// Errors retain output already written to `out`. Like `run`, this API reports
/// name errors at runtime; callers wanting CLI-style static checks can call
/// `resolve::check` separately.
///
/// ```
/// use limits::{run_with_engine, Engine, interp::Limits};
/// let mut out = Vec::new();
/// run_with_engine("print(42);", &mut out, Limits::default(), Engine::Vm).unwrap();
/// assert_eq!(out, b"42\n");
/// ```
pub fn run_with_engine(
    src: &str,
    out: &mut dyn std::io::Write,
    limits: interp::Limits,
    engine: Engine,
) -> Result<(), LimError> {
    let program = parser::parse(lexer::lex(src)?)?;
    match engine {
        Engine::Tree => interp::Interpreter::with_limits(out, limits).run(&program),
        Engine::Vm => vm::Vm::with_limits(out, limits).run(codegen::compile(&program)?),
    }
}

pub mod sandbox;

pub mod semantic;

pub mod protocol;

pub mod sealed;

pub mod stdlib;
