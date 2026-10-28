use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Lex,
    Parse,
    Runtime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LimError {
    pub kind: Kind,
    pub msg: String,
    pub span: Span,
}

impl LimError {
    pub fn new(kind: Kind, msg: impl Into<String>, span: Span) -> Self {
        LimError {
            kind,
            msg: msg.into(),
            span,
        }
    }
}

impl fmt::Display for LimError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let k = match self.kind {
            Kind::Lex => "lex",
            Kind::Parse => "parse",
            Kind::Runtime => "runtime",
        };
        write!(
            f,
            "{} error at {}:{}: {}",
            k, self.span.line, self.span.col, self.msg
        )
    }
}
