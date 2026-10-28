//! Lexer: source text to tokens. Single pass over bytes.

use crate::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),
    Kw(&'static str),
    Sym(&'static str),
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
    pub col: u32,
}

const KEYWORDS: &[&str] = &[
    "let", "fn", "return", "if", "else", "while", "for", "in", "true", "false", "and", "or", "not",
    "break", "continue", "struct", "import", "pub",
];

// Longest first so that two-char symbols win.
const SYMS: &[&str] = &[
    "..", "==", "!=", "<=", ">=", "->", "(", ")", "{", "}", "[", "]", ",", ";", ":", "+", "-", "*",
    "/", "%", "=", "<", ">", ".",
];

pub fn lex(src: &str) -> Result<Vec<Token>, Error> {
    let b = src.as_bytes();
    let (mut i, mut line, mut col) = (0usize, 1u32, 1u32);
    let mut out = Vec::new();
    let err = |l, c, m: String| Error::new("lex", l, c, m);
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            i += 1;
            line += 1;
            col = 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            col += 1;
            continue;
        }
        if c == b'#' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let (sl, sc, start) = (line, col, i);
        let tok = if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                i += 1;
            }
            let is_float = i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit();
            if is_float {
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let text: String = src[start..i].chars().filter(|&c| c != '_').collect();
            if is_float {
                Tok::Float(text.parse().unwrap())
            } else {
                // 2^63 is only valid right after a unary minus; it is lexed as
                // i64::MIN and the parser rejects it anywhere else.
                match text.parse::<i64>() {
                    Ok(n) => Tok::Int(n),
                    Err(_) if text == "9223372036854775808" => Tok::Int(i64::MIN),
                    Err(_) => {
                        return Err(err(
                            sl,
                            sc,
                            format!("integer literal '{}' out of range", text),
                        ))
                    }
                }
            }
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let w = &src[start..i];
            let kw = if w.len() <= 8 && w.as_bytes()[0].is_ascii_lowercase() {
                KEYWORDS.iter().find(|k| **k == w)
            } else {
                None
            };
            match kw {
                Some(k) => Tok::Kw(k),
                None => Tok::Ident(w.to_string()),
            }
        } else if c == b'"' {
            i += 1;
            let mut s = Vec::new();
            loop {
                match b.get(i) {
                    None | Some(b'\n') => return Err(err(sl, sc, "unterminated string".into())),
                    Some(b'"') => break,
                    Some(b'\\') => {
                        let e = match b.get(i + 1) {
                            Some(b'n') => b'\n',
                            Some(b't') => b'\t',
                            Some(b'\\') => b'\\',
                            Some(b'"') => b'"',
                            _ => {
                                return Err(err(
                                    line,
                                    col + (i - start) as u32,
                                    "unknown escape".into(),
                                ))
                            }
                        };
                        s.push(e);
                        i += 2;
                    }
                    Some(&ch) => {
                        s.push(ch);
                        i += 1;
                    }
                }
            }
            i += 1;
            Tok::Str(String::from_utf8(s).unwrap())
        } else if let Some(s) = SYMS.iter().find(|s| b[i..].starts_with(s.as_bytes())) {
            i += s.len();
            Tok::Sym(s)
        } else {
            let ch = src[i..].chars().next().unwrap();
            return Err(err(sl, sc, format!("unexpected character '{}'", ch)));
        };
        // Only string literals can contain non-ASCII text.
        col += if c == b'"' {
            src[start..i].chars().count()
        } else {
            i - start
        } as u32;
        out.push(Token {
            tok,
            line: sl,
            col: sc,
        });
    }
    out.push(Token {
        tok: Tok::Eof,
        line,
        col,
    });
    Ok(out)
}
