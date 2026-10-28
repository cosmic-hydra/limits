//! Lexer: source text to tokens. Single pass over bytes. Blocks are
//! indentation based: the lexer emits `Newline` at the end of each logical
//! line and `Indent`/`Dedent` when the indentation (4 spaces per level)
//! changes. Inside `()`, `[]` and `{}` line breaks and indentation are ignored.

use crate::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),
    Kw(&'static str),
    Sym(&'static str),
    Newline,
    Indent,
    Dedent,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
    pub col: u32,
}

const KEYWORDS: &[&str] = &[
    "let", "var", "fn", "return", "if", "elif", "else", "while", "for", "in", "true", "false",
    "and", "or", "not", "break", "continue", "pass", "struct", "import", "pub", "enum", "match",
];

// Longest first so that two-char symbols win.
const SYMS: &[&str] = &[
    "..", "==", "!=", "<=", ">=", "->", "+=", "-=", "*=", "/=", "%=", "(", ")", "{", "}", "[", "]",
    ",", ";", ":", "+", "-", "*", "/", "%", "=", "<", ">", ".", "?", "&",
];

pub fn lex(src: &str) -> Result<Vec<Token>, Error> {
    let b = src.as_bytes();
    let (mut i, mut line, mut col) = (0usize, 1u32, 1u32);
    let mut out = Vec::new();
    let err = |l, c, m: String| Error::new("lex", l, c, m);
    // Indentation stack (in spaces), bracket depth, and whether we are at the
    // start of a line / have emitted a token on the current logical line.
    let mut indents = vec![0usize];
    let mut depth = 0usize;
    let mut at_start = true;
    let mut has_tok = false;
    let push = |out: &mut Vec<Token>, tok, line, col| out.push(Token { tok, line, col });
    while i < b.len() {
        if at_start && depth == 0 {
            let mut n = 0;
            while i + n < b.len() && b[i + n] == b' ' {
                n += 1;
            }
            let next = b.get(i + n).copied();
            if next == Some(b'\t') {
                return Err(err(
                    line,
                    n as u32 + 1,
                    "tab character: indent with 4 spaces".into(),
                ));
            }
            // Blank and comment-only lines do not affect indentation.
            if !matches!(next, None | Some(b'\n') | Some(b'\r') | Some(b'#')) {
                let top = *indents.last().unwrap();
                let (l, c) = (line, n as u32 + 1);
                if n > top {
                    if n != top + 4 {
                        let m = format!(
                            "indentation must be 4 spaces per level (expected {} spaces, got {})",
                            top + 4,
                            n
                        );
                        return Err(err(l, c, m));
                    }
                    indents.push(n);
                    push(&mut out, Tok::Indent, l, c);
                } else {
                    while n < *indents.last().unwrap() {
                        indents.pop();
                        push(&mut out, Tok::Dedent, l, c);
                    }
                    if n != *indents.last().unwrap() {
                        let m = "unindent does not match any outer indentation level".into();
                        return Err(err(l, c, m));
                    }
                }
            }
            at_start = false;
            i += n;
            col += n as u32;
            continue;
        }
        let c = b[i];
        if c == b'\n' {
            if depth == 0 {
                if has_tok {
                    push(&mut out, Tok::Newline, line, col);
                    has_tok = false;
                }
                at_start = true;
            }
            i += 1;
            line += 1;
            col = 1;
            continue;
        }
        if c == b'\t' {
            return Err(err(line, col, "tab character: indent with 4 spaces".into()));
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
            match *s {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth = depth.saturating_sub(1),
                _ => {}
            }
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
        has_tok = true;
        push(&mut out, tok, sl, sc);
    }
    if has_tok {
        push(&mut out, Tok::Newline, line, col);
    }
    for _ in 1..indents.len() {
        push(&mut out, Tok::Dedent, line, col);
    }
    push(&mut out, Tok::Eof, line, col);
    Ok(out)
}
