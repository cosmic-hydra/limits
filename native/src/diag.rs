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
    let dist = |a: &str, b: &str| {
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
    };
    let max = name.chars().count() / 3;
    cands
        .filter(|c| *c != name)
        .map(|c| (dist(name, c), c))
        .filter(|(d, _)| *d <= max)
        .min()
        .map_or(String::new(), |(_, c)| format!("did you mean '{}'?", c))
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

/// One diagnostic as a single-line JSON object.
pub fn json(kind: &str, file: &str, line: u32, col: u32, msg: &str, hint: &str) -> String {
    format!(
        "{{\"kind\":\"{}\",\"code\":\"{}\",\"file\":{},\"line\":{},\"col\":{},\"msg\":{},\"hint\":{}}}",
        kind,
        code(kind, msg),
        json_str(file),
        line,
        col,
        json_str(msg),
        json_str(hint)
    )
}
