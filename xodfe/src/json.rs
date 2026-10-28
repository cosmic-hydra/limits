//! Minimal JSON parser/serializer, just enough for safetensors headers.
//! Numbers are kept as their source text so u64 offsets stay exact.

use crate::error::{fail, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Num(s) => s.parse().ok(),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
}

const MAX_DEPTH: usize = 64;

pub fn parse(src: &[u8]) -> Result<Json> {
    let mut p = P { s: src, i: 0 };
    let v = p.value(0)?;
    p.ws();
    if p.i != src.len() {
        return fail(format!("json: trailing characters at {}", p.i));
    }
    Ok(v)
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn eat(&mut self, c: u8) -> Result<()> {
        self.ws();
        if self.peek() != Some(c) {
            return fail(format!("json: expected '{}' at {}", c as char, self.i));
        }
        self.i += 1;
        Ok(())
    }
    fn lit(&mut self, w: &str, v: Json) -> Result<Json> {
        if self.s[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
            Ok(v)
        } else {
            fail(format!("json: bad literal at {}", self.i))
        }
    }
    fn value(&mut self, depth: usize) -> Result<Json> {
        if depth > MAX_DEPTH {
            return fail("json: nesting too deep");
        }
        self.ws();
        match self.peek() {
            Some(b'{') => {
                self.i += 1;
                let mut kv = Vec::new();
                self.ws();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(kv));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.eat(b':')?;
                    kv.push((k, self.value(depth + 1)?));
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(kv));
                        }
                        _ => return fail(format!("json: expected ',' or '}}' at {}", self.i)),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.peek() == Some(b']') {
                    self.i += 1;
                    return Ok(Json::Arr(v));
                }
                loop {
                    v.push(self.value(depth + 1)?);
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(v));
                        }
                        _ => return fail(format!("json: expected ',' or ']' at {}", self.i)),
                    }
                }
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'n') => self.lit("null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => {
                let st = self.i;
                while self
                    .peek()
                    .is_some_and(|c| c.is_ascii_digit() || b"+-.eE".contains(&c))
                {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[st..self.i]).unwrap_or("");
                if t.parse::<f64>().is_err() {
                    return fail(format!("json: bad number at {st}"));
                }
                Ok(Json::Num(t.to_string()))
            }
            _ => fail(format!("json: unexpected input at {}", self.i)),
        }
    }
    fn hex4(&mut self) -> Result<u32> {
        let h = self
            .s
            .get(self.i..self.i + 4)
            .and_then(|h| std::str::from_utf8(h).ok());
        let v = h.and_then(|h| u32::from_str_radix(h, 16).ok());
        self.i += 4;
        v.map_or_else(|| fail("json: bad \\u escape"), Ok)
    }
    fn string(&mut self) -> Result<String> {
        if self.peek() != Some(b'"') {
            return fail(format!("json: expected string at {}", self.i));
        }
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                return fail("json: unterminated string");
            };
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let Some(e) = self.peek() else {
                        return fail("json: bad escape");
                    };
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let mut u = self.hex4()?;
                            if (0xD800..0xDC00).contains(&u) && self.s[self.i..].starts_with(b"\\u")
                            {
                                self.i += 2;
                                let lo = self.hex4()?;
                                u = 0x10000
                                    + ((u - 0xD800) << 10)
                                    + (lo.wrapping_sub(0xDC00) & 0x3ff);
                            }
                            char::from_u32(u).unwrap_or('\u{FFFD}')
                        }
                        _ => return fail("json: bad escape"),
                    };
                    out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                }
                c => out.push(c),
            }
        }
        String::from_utf8(out).or_else(|_| fail("json: invalid UTF-8 in string"))
    }
}

/// Serialize a string with JSON escaping.
pub fn quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}
