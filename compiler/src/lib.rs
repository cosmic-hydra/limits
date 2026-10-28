//! Limits: a small, explicit language for AI agents.
//! Pipeline: source -> lexer -> parser (AST) -> tree-walking interpreter.

pub mod ast;
pub mod error;
pub mod interp;
pub mod lexer;
pub mod parser;

use error::LimError;

/// Lex, parse, and run a program. Output from `print` goes to `out`.
pub fn run(src: &str, out: &mut dyn std::io::Write) -> Result<(), LimError> {
    let tokens = lexer::lex(src)?;
    let program = parser::parse(tokens)?;
    interp::Interpreter::new(out).run(&program)
}
