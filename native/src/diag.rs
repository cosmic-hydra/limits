//! Diagnostic codes, hints and JSON output. Codes are assigned from the
//! message text through one table, which is also embedded into generated
//! programs (for runtime errors in `--json` mode) and documented in SPEC.md.

/// (kind, message fragment, code). The first row whose kind matches and whose
/// fragment occurs in the message wins, so specific rows come first.
pub const CODES: &[(&str, &str, &str)] = &[
    ("lex", "out of range", "E0001"),
    ("lex", "unterminated string", "E0002"),
    ("lex", "unknown escape", "E0003"),
    ("lex", "unexpected character", "E0004"),
    ("lex", "invalid UTF-8", "E0005"),
    ("lex", "tab character", "E0006"),
    ("lex", "indent", "E0007"),
    ("parse", "nesting too deep", "E0101"),
    ("parse", "comparison operators cannot be chained", "E0102"),
    ("parse", "nested definitions", "E0103"),
    ("parse", "invalid assignment target", "E0104"),
    ("parse", "", "E0100"),
    ("check", "cannot infer the type of '", "E0316"),
    ("check", "is not exhaustive", "E0317"),
    ("check", "unreachable match arm", "E0318"),
    ("check", "has no variant", "E0319"),
    ("check", "but the variant has", "E0319"),
    ("check", "cannot match on", "E0319"),
    ("check", "pattern ", "E0319"),
    ("check", "'?' ", "E0320"),
    ("check", "match arm must end with a value", "E0321"),
    ("check", "undefined variable", "E0201"),
    ("check", "assignment to undeclared variable", "E0202"),
    ("check", "undefined function", "E0203"),
    ("check", ", not a function", "E0228"),
    ("check", "unknown struct", "E0204"),
    ("check", "unknown type", "E0204"),
    ("check", "unknown module", "E0205"),
    ("check", "' has no function '", "E0206"),
    ("check", "' has no struct '", "E0206"),
    ("check", "is private", "E0207"),
    ("check", "defined twice", "E0208"),
    ("check", "already declared in this block", "E0209"),
    ("check", "duplicate parameter", "E0210"),
    ("check", "duplicate field", "E0211"),
    ("check", "is a builtin", "E0212"),
    ("check", "missing 'fn main()'", "E0213"),
    ("check", "'main' must take", "E0214"),
    ("check", "cannot find module", "E0215"),
    ("check", "import cycle", "E0216"),
    ("check", "imported twice", "E0217"),
    ("check", "outside of a loop", "E0218"),
    ("check", "needs capability", "E0219"),
    ("check", "unknown capability", "E0220"),
    ("check", "not declared by the root file", "E0221"),
    ("check", "cannot assign to", "E0222"),
    ("check", "cannot change", "E0222"),
    ("check", "can only change a variable", "E0222"),
    ("check", "inout", "E0225"),
    ("check", "cannot be stored", "E0226"),
    ("check", "has no method", "E0227"),
    (
        "check",
        "cannot be combined with top-level statements",
        "E0224",
    ),
    ("check", "top-level statements", "E0223"),
    ("check", "' literal", "E0314"),
    ("check", "has no fields", "E0306"),
    ("check", "has no field", "E0305"),
    ("check", "argument(s), got", "E0303"),
    ("check", "' expects ", "E0304"),
    ("check", "cannot apply", "E0302"),
    ("check", "cannot index", "E0307"),
    ("check", "can only iterate", "E0308"),
    ("check", "empty list", "E0309"),
    ("check", "cannot bind a unit value", "E0310"),
    ("check", "may finish without returning", "E0311"),
    ("check", "'return' has a value", "E0312"),
    ("check", "missing return value", "E0313"),
    ("check", "contains itself", "E0315"),
    ("check", " must be ", "E0301"),
    ("check", "", "E0300"),
    ("runtime", "unwrap of ", "E0517"),
    ("runtime", "access denied", "E0514"),
    ("runtime", "cannot read file", "E0515"),
    ("runtime", "cannot write file", "E0515"),
    ("runtime", "cannot use directory", "E0515"),
    ("runtime", "out of bounds", "E0501"),
    ("runtime", "division by zero", "E0502"),
    ("runtime", "integer overflow", "E0503"),
    ("runtime", "pop from empty list", "E0504"),
    ("runtime", "of empty list", "E0516"),
    ("runtime", "out of int range", "E0505"),
    ("runtime", "literal", "E0505"),
    ("runtime", "assertion failed", "E0506"),
    ("runtime", "step limit", "E0507"),
    ("runtime", "call depth limit", "E0508"),
    ("runtime", "memory", "E0509"),
    ("runtime", "nested too deeply", "E0510"),
    ("runtime", "requires failed", "E0511"),
    ("runtime", "ensures failed", "E0512"),
    ("runtime", "killed by signal", "E0513"),
    ("runtime", "", "E0500"),
];

pub fn code(kind: &str, msg: &str) -> &'static str {
    CODES
        .iter()
        .find(|(k, f, _)| *k == kind && msg.contains(f))
        .map_or("E0000", |r| r.2)
}

/// `"did you mean 'x'?"` for the candidate closest to `name` by edit
/// distance, if one is close enough to be a plausible typo.
pub fn suggest<'a>(name: &str, cands: impl Iterator<Item = &'a str>) -> String {
    let max = name.chars().count() / 3;
    cands
        .filter(|c| *c != name)
        .map(|c| (dist(name, c), c))
        .filter(|(d, _)| *d <= max)
        .min()
        .map_or(String::new(), |(_, c)| format!("did you mean '{}'?", c))
}

/// The valid names closest to `name` (by edit distance, then name), at most
/// 10: the `alternatives` of a diagnostic about an unknown name.
pub fn ranked<'a>(name: &str, cands: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut v: Vec<(usize, &str)> = cands
        .filter(|c| *c != name && !c.starts_with('$') && !c.is_empty())
        .map(|c| (dist(name, c), c))
        .collect();
    v.sort();
    v.dedup();
    v.into_iter().take(10).map(|(_, c)| c.to_string()).collect()
}

/// Levenshtein distance over chars.
fn dist(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for j in 0..b.len() {
            let cur = row[j + 1];
            row[j + 1] = (prev + (ca != b[j]) as usize).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

pub fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// One text edit: replace the span from (line, col) up to (end_line,
/// end_col) (exclusive; columns count code points from 1) with `text`.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub text: String,
}

impl Edit {
    /// Insert `text` before (line, col).
    pub fn insert(line: u32, col: u32, text: String) -> Self {
        Edit {
            line,
            col,
            end_line: line,
            end_col: col,
            text,
        }
    }
    /// Replace `n` code points on one line.
    pub fn replace(line: u32, col: u32, n: u32, text: String) -> Self {
        Edit {
            line,
            col,
            end_line: line,
            end_col: col + n,
            text,
        }
    }
}

/// The optional parts of a diagnostic.
#[derive(Debug, Clone, Default)]
pub struct More {
    /// Span end (line, exclusive col); filled by `enrich` if unset.
    pub end: Option<(u32, u32)>,
    /// Expected and found type or token kind, where the error is a mismatch.
    pub expected: String,
    pub found: String,
    /// Valid names/variants/types that could stand here.
    pub alts: Vec<String>,
    pub fixes: Vec<Fix>,
}

/// A suggested fix. `machine`: applying it is safe without review (`limits
/// fix` applies these); otherwise it is a plausible repair ("maybe").
#[derive(Debug, Clone, PartialEq)]
pub struct Fix {
    pub msg: String,
    pub edits: Vec<Edit>,
    pub machine: bool,
}

/// Line `l` (1-based) of `src`, without its line break.
pub fn line_of(src: &str, l: u32) -> &str {
    src.split('\n')
        .nth(l.saturating_sub(1) as usize)
        .map_or("", |s| s.strip_suffix('\r').unwrap_or(s))
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// End column (exclusive) of the token starting at `col` on `line`.
fn token_end(line: &str, col: u32) -> u32 {
    let cs: Vec<char> = line.chars().collect();
    let i = col.saturating_sub(1) as usize;
    let Some(&c) = cs.get(i) else {
        return col;
    };
    let mut j = i + 1;
    if is_word(c) {
        while j < cs.len() && (is_word(cs[j]) || (cs[j] == '.' && c.is_ascii_digit())) {
            j += 1;
        }
    } else if c == '"' {
        while j < cs.len() && cs[j] != '"' {
            j += if cs[j] == '\\' { 2 } else { 1 };
        }
        j = (j + 1).min(cs.len());
    } else if j < cs.len() {
        let two: String = cs[i..=j].iter().collect();
        if [
            "..", "==", "!=", "<=", ">=", "->", "+=", "-=", "*=", "/=", "%=",
        ]
        .contains(&two.as_str())
        {
            j += 1;
        }
    }
    j as u32 + 1
}

/// Fill in what the checker did not set, from the message and the source:
/// the span end, `expected`/`found` for mismatches, and the did-you-mean fix.
pub fn enrich(e: &mut crate::Error, src: Option<&str>) {
    let line = src.map_or("", |s| line_of(s, e.line));
    if e.more.end.is_none() {
        let end = if e.line == 0 {
            (0, 0)
        } else {
            (e.line, token_end(line, e.col))
        };
        e.more.end = Some(end);
    }
    if e.more.found.is_empty() {
        let m = e.msg.as_str();
        if let Some(k) = m.rfind(", got ") {
            e.more.found = m[k + 6..].to_string();
            let head = &m[..k];
            for pre in [
                " must be ",
                " expects ",
                " takes ",
                "can only iterate over ",
                "needs ",
            ] {
                if let Some(p) = head.rfind(pre) {
                    e.more.expected = head[p + pre.len()..].to_string();
                    break;
                }
            }
            if head.contains(" takes ") {
                e.more.found = format!("{} argument(s)", e.more.found);
            }
        } else if let Some(k) = m.find("cannot apply '") {
            let op = m[k + 14..].split('\'').next().unwrap_or("");
            if let Some(p) = m[k..].find(" to ") {
                let f = &m[k + p + 4..];
                e.more.found = f.split(" (").next().unwrap_or(f).to_string();
                let binary = e.more.found.contains(" and ");
                e.more.expected = match op {
                    "+" | "+=" => "int, float or str on both sides",
                    "and" | "or" | "not" => "bool",
                    "==" | "!=" => "the same int, float, str or bool type on both sides",
                    "<" | "<=" | ">" | ">=" => "the same int, float or str type on both sides",
                    _ if binary => "int or float on both sides",
                    _ => "int or float",
                }
                .to_string();
            }
        }
    }
    // Did-you-mean: rename the misspelled name (the last quoted name in the
    // message) at or after the error column.
    if let Some(k) = e.hint.find("did you mean '") {
        let rest = &e.hint[k + 14..];
        let good = rest.split('\'').next().unwrap_or("").to_string();
        let bad = e.msg.rsplit('\'').nth(1).unwrap_or("").to_string();
        if !good.is_empty() && !e.more.alts.contains(&good) {
            e.more.alts.insert(0, good.clone());
        }
        let have = e.more.fixes.iter().any(|f| f.msg.starts_with("rename"));
        if !have && !bad.is_empty() && !good.is_empty() {
            if let Some(c) = find_word(line, &bad, e.col) {
                let n = bad.chars().count() as u32;
                e.more.fixes.push(Fix {
                    msg: format!("rename '{}' to '{}'", bad, good),
                    edits: vec![Edit::replace(e.line, c, n, good)],
                    machine: true,
                });
            }
        }
    }
}

/// Column of the first whole-word occurrence of `w` at or after `col`.
fn find_word(line: &str, w: &str, col: u32) -> Option<u32> {
    let cs: Vec<char> = line.chars().collect();
    let wc: Vec<char> = w.chars().collect();
    let start = col.saturating_sub(1) as usize;
    (start..cs.len()).find_map(|i| {
        let ok = cs[i..].starts_with(&wc)
            && (i == 0 || !is_word(cs[i - 1]))
            && cs.get(i + wc.len()).is_none_or(|c| !is_word(*c));
        ok.then_some(i as u32 + 1)
    })
}

fn opt_str(s: &str) -> String {
    if s.is_empty() {
        "null".into()
    } else {
        json_str(s)
    }
}

/// One diagnostic as a single-line JSON object (call `enrich` first).
pub fn json_err(e: &crate::Error, file: &str, src: Option<&str>) -> String {
    let (el, ec) = e.more.end.unwrap_or((e.line, e.col));
    let alts: Vec<String> = e.more.alts.iter().map(|a| json_str(a)).collect();
    let fixes: Vec<String> = e.more.fixes.iter().map(fix_json).collect();
    format!(
        "{{\"kind\":\"{}\",\"code\":\"{}\",\"file\":{},\"line\":{},\"col\":{},\"end_line\":{},\"end_col\":{},\"msg\":{},\"hint\":{},\"source_line\":{},\"expected\":{},\"found\":{},\"alternatives\":[{}],\"fixes\":[{}]}}",
        e.kind,
        code(e.kind, &e.msg),
        json_str(file),
        e.line,
        e.col,
        el,
        ec,
        json_str(&e.msg),
        json_str(&e.hint),
        json_str(src.map_or("", |s| line_of(s, e.line))),
        opt_str(&e.more.expected),
        opt_str(&e.more.found),
        alts.join(","),
        fixes.join(",")
    )
}

pub fn fix_json(f: &Fix) -> String {
    let edits: Vec<String> = f
        .edits
        .iter()
        .map(|d| {
            format!(
                "{{\"line\":{},\"col\":{},\"end_line\":{},\"end_col\":{},\"text\":{}}}",
                d.line,
                d.col,
                d.end_line,
                d.end_col,
                json_str(&d.text)
            )
        })
        .collect();
    format!(
        "{{\"msg\":{},\"edits\":[{}],\"applicability\":\"{}\"}}",
        json_str(&f.msg),
        edits.join(","),
        if f.machine { "machine" } else { "maybe" }
    )
}

/// A runtime-style diagnostic with only a message (signals, test timeouts).
pub fn json(kind: &str, file: &str, line: u32, col: u32, msg: &str, hint: &str) -> String {
    let mut e = crate::Error::new(kind_static(kind), line, col, msg.to_string());
    e.hint = hint.to_string();
    e.more.end = Some((line, col));
    json_err(&e, file, None)
}

fn kind_static(k: &str) -> &'static str {
    ["lex", "parse", "check", "runtime"]
        .into_iter()
        .find(|x| *x == k)
        .unwrap_or("runtime")
}

/// Byte offset of (line, col) in `src` (col counts code points).
pub fn offset(src: &str, line: u32, col: u32) -> usize {
    let mut off = 0;
    for (i, l) in src.split_inclusive('\n').enumerate() {
        if i + 1 == line as usize {
            let body = l.strip_suffix('\n').unwrap_or(l);
            return off
                + body
                    .char_indices()
                    .nth(col.saturating_sub(1) as usize)
                    .map_or(body.len(), |(b, _)| b);
        }
        off += l.len();
    }
    src.len()
}

/// Apply non-overlapping edits (overlapping ones after the first are
/// skipped). Returns the new text and the number of edits applied.
pub fn apply(src: &str, edits: &[Edit]) -> (String, usize) {
    let mut spans: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| {
            let a = offset(src, e.line, e.col);
            let b = offset(src, e.end_line, e.end_col).max(a);
            (a, b, e.text.as_str())
        })
        .collect();
    spans.sort_by_key(|s| (s.0, s.1));
    let mut out = String::with_capacity(src.len() + 16);
    let (mut pos, mut n) = (0, 0);
    for (a, b, t) in spans {
        if a < pos {
            continue;
        }
        out.push_str(&src[pos..a]);
        out.push_str(t);
        pos = b;
        n += 1;
    }
    out.push_str(&src[pos..]);
    (out, n)
}
