use crate::error::{Kind, LimError, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),
    // keywords
    Let,
    Fn,
    Return,
    If,
    Else,
    While,
    For,
    In,
    True,
    False,
    Nil,
    And,
    Or,
    Not,
    Break,
    Continue,
    // punctuation / operators
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Assign,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    DotDot,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

/// Maximum source bytes accepted by both library and CLI entry points.
pub const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;

pub fn lex(src: &str) -> Result<Vec<Token>, LimError> {
    if src.len() > MAX_SOURCE_BYTES {
        return Err(LimError::new(
            Kind::Lex,
            "source size limit exceeded (16777216 bytes)",
            Span { line: 1, col: 1 },
        ));
    }
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let (mut i, mut line, mut col) = (0usize, 1usize, 1usize);

    macro_rules! adv {
        () => {{
            if chars[i] == '\r' {
                line += 1;
                col = 1;
            } else if chars[i] == '\n' {
                if i == 0 || chars[i - 1] != '\r' {
                    line += 1;
                }
                col = 1;
            } else {
                col += 1;
            }
            i += 1;
        }};
    }

    while i < chars.len() {
        let c = chars[i];
        let span = Span { line, col };
        if c.is_whitespace() {
            adv!();
            continue;
        }
        if c == '#' {
            while i < chars.len() && !matches!(chars[i], '\n' | '\r') {
                adv!();
            }
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                adv!();
            }
            let mut is_float = false;
            // `1..5` is a range, not a float
            if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                is_float = true;
                adv!();
                while i < chars.len() && chars[i].is_ascii_digit() {
                    adv!();
                }
            }
            let text: String = chars[start..i].iter().collect();
            let tok = if is_float {
                Tok::Float(text.parse().unwrap())
            } else {
                Tok::Int(
                    text.parse()
                        .map_err(|_| LimError::new(Kind::Lex, "integer literal too large", span))?,
                )
            };
            toks.push(Token { tok, span });
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                adv!();
            }
            let word: String = chars[start..i].iter().collect();
            let tok = match word.as_str() {
                "let" => Tok::Let,
                "fn" => Tok::Fn,
                "return" => Tok::Return,
                "if" => Tok::If,
                "else" => Tok::Else,
                "while" => Tok::While,
                "for" => Tok::For,
                "in" => Tok::In,
                "true" => Tok::True,
                "false" => Tok::False,
                "nil" => Tok::Nil,
                "and" => Tok::And,
                "or" => Tok::Or,
                "not" => Tok::Not,
                "break" => Tok::Break,
                "continue" => Tok::Continue,
                _ => Tok::Ident(word),
            };
            toks.push(Token { tok, span });
            continue;
        }
        if c == '"' {
            adv!();
            let mut s = String::new();
            loop {
                if i >= chars.len() {
                    return Err(LimError::new(Kind::Lex, "unterminated string", span));
                }
                let ch = chars[i];
                if ch == '"' {
                    adv!();
                    break;
                }
                if ch == '\\' {
                    adv!();
                    if i >= chars.len() {
                        return Err(LimError::new(Kind::Lex, "unterminated string", span));
                    }
                    let esc = match chars[i] {
                        'n' => '\n',
                        't' => '\t',
                        '\\' => '\\',
                        '"' => '"',
                        other => {
                            return Err(LimError::new(
                                Kind::Lex,
                                format!("unknown escape \\{}", other),
                                Span { line, col },
                            ))
                        }
                    };
                    s.push(esc);
                    adv!();
                    continue;
                }
                s.push(ch);
                adv!();
            }
            toks.push(Token {
                tok: Tok::Str(s),
                span,
            });
            continue;
        }
        let next = chars.get(i + 1).copied();
        let (tok, len) = match (c, next) {
            ('=', Some('=')) => (Tok::Eq, 2),
            ('!', Some('=')) => (Tok::Ne, 2),
            ('<', Some('=')) => (Tok::Le, 2),
            ('>', Some('=')) => (Tok::Ge, 2),
            ('.', Some('.')) => (Tok::DotDot, 2),
            ('(', _) => (Tok::LParen, 1),
            (')', _) => (Tok::RParen, 1),
            ('{', _) => (Tok::LBrace, 1),
            ('}', _) => (Tok::RBrace, 1),
            ('[', _) => (Tok::LBracket, 1),
            (']', _) => (Tok::RBracket, 1),
            (',', _) => (Tok::Comma, 1),
            (';', _) => (Tok::Semi, 1),
            (':', _) => (Tok::Colon, 1),
            ('+', _) => (Tok::Plus, 1),
            ('-', _) => (Tok::Minus, 1),
            ('*', _) => (Tok::Star, 1),
            ('/', _) => (Tok::Slash, 1),
            ('%', _) => (Tok::Percent, 1),
            ('=', _) => (Tok::Assign, 1),
            ('<', _) => (Tok::Lt, 1),
            ('>', _) => (Tok::Gt, 1),
            _ => {
                return Err(LimError::new(
                    Kind::Lex,
                    format!("unexpected character '{}'", c),
                    span,
                ))
            }
        };
        for _ in 0..len {
            adv!();
        }
        toks.push(Token { tok, span });
    }
    toks.push(Token {
        tok: Tok::Eof,
        span: Span { line, col },
    });
    Ok(toks)
}
