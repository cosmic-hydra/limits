//! Support for type-guided generation: checking an unfinished program prefix
//! (`limits check --partial`) and describing what may come next at its end
//! (`--expect`). Both work on text only and reuse the real lexer, parser and
//! checker: the prefix is completed with the smallest neutral suffix (closing
//! strings and brackets, `pass` bodies), and errors that only the early end
//! of the input causes are dropped.

use crate::ast::{Cap, Ty};
use crate::check::{self, Hole, HOLE};
use crate::diag::json_str;
use crate::{lexer, module, parser, Error};
use std::path::Path;

/// Per-line layout of a source text.
struct Lines {
    /// Bracket depth at the start of each line (outside strings/comments).
    depth: Vec<usize>,
    /// Leading spaces, or None for a blank or comment-only line.
    indent: Vec<Option<usize>>,
}

/// State at the end of a text: open brackets (innermost last), and whether
/// it ends inside a string (and right after a backslash) or a comment.
struct End {
    brackets: Vec<char>,
    in_str: bool,
    esc: bool,
    in_comment: bool,
}

fn scan(src: &str) -> (Lines, End) {
    let mut lines = Lines {
        depth: vec![0],
        indent: Vec::new(),
    };
    let mut end = End {
        brackets: Vec::new(),
        in_str: false,
        esc: false,
        in_comment: false,
    };
    for line in src.split('\n') {
        let t = line.trim_start_matches(' ');
        lines.indent.push(if t.is_empty() || t.starts_with('#') {
            None
        } else {
            Some(line.len() - t.len())
        });
        end.in_str = false;
        end.in_comment = false;
        end.esc = false;
        for ch in line.chars() {
            if end.in_comment {
                break;
            }
            if end.in_str {
                match ch {
                    _ if end.esc => end.esc = false,
                    '\\' => end.esc = true,
                    '"' => end.in_str = false,
                    _ => {}
                }
                continue;
            }
            match ch {
                '"' => end.in_str = true,
                '#' => end.in_comment = true,
                '(' | '[' | '{' => end.brackets.push(ch),
                ')' | ']' | '}' => {
                    end.brackets.pop();
                }
                _ => {}
            }
        }
        lines.depth.push(end.brackets.len());
    }
    lines.depth.pop();
    (lines, end)
}

impl Lines {
    /// Line (1-based) where the last logical line starts: the last
    /// non-blank line, moved back to a line that starts outside brackets.
    fn last_logical(&self) -> u32 {
        let Some(mut i) = (0..self.indent.len())
            .rev()
            .find(|&i| self.indent[i].is_some())
        else {
            return self.indent.len() as u32 + 1;
        };
        while i > 0 && self.depth[i] > 0 {
            i -= 1;
        }
        i as u32 + 1
    }

    /// Lines whose block is still open at the end of the input: every later
    /// non-blank line (outside brackets) is indented deeper.
    fn open(&self) -> Vec<bool> {
        let mut out = vec![false; self.indent.len()];
        let mut min_later = usize::MAX;
        for i in (0..self.indent.len()).rev() {
            if self.depth[i] > 0 {
                continue;
            }
            if let Some(k) = self.indent[i] {
                out[i] = min_later > k;
                min_later = min_later.min(k);
            }
        }
        out
    }
}

fn closer(c: char) -> char {
    match c {
        '(' => ')',
        '[' => ']',
        _ => '}',
    }
}

/// Text that closes an open string and the open brackets at the end of `src`.
fn closers(end: &End) -> String {
    let mut s = String::new();
    if end.in_str {
        s.push_str(if end.esc { "n\"" } else { "\"" });
    }
    s.extend(end.brackets.iter().rev().map(|&c| closer(c)));
    s
}

/// The last line of `src` with code on it.
fn last_code_line(src: &str) -> Option<&str> {
    src.rsplit('\n').find(|l| {
        let t = l.trim_start();
        !t.is_empty() && !t.starts_with('#')
    })
}

/// A body for a block header that has none yet (`if x:`, `match v:`).
fn filler(header: &str) -> String {
    let ind = header.len() - header.trim_start().len() + 4;
    let code = header.split(" #").next().unwrap_or(header).trim();
    let body = if code.starts_with("struct ") || code.starts_with("pub struct ") {
        "lim_pad_: int"
    } else if code.starts_with("enum ") || code.starts_with("pub enum ") {
        "LimPad_"
    } else if code.starts_with("match ") || code.contains(" match ") {
        "_: pass"
    } else {
        "pass"
    };
    format!("\n{}{}\n", " ".repeat(ind), body)
}

fn ends_block(line: &str) -> bool {
    line.split(" #")
        .next()
        .unwrap_or(line)
        .trim_end()
        .ends_with(':')
}

/// Codes of check errors that an unfinished block can cause (missing
/// return, arms or arm value; no entry point yet).
const OPEN_CODES: &[&str] = &["E0311", "E0317", "E0321", "E0213"];

/// `check --partial`: the errors of an unfinished program prefix, without
/// those caused only by the input ending early.
pub fn partial(root: &Path, src: &str, need_main: bool) -> Vec<Error> {
    let (lines, end) = scan(src);
    let nlines = lines.indent.len() as u32;
    // Is the last line unfinished? (No final newline, or inside a string or
    // brackets.) Errors from it are not reported.
    let unfinished = !src.ends_with('\n') || end.in_str || !end.brackets.is_empty();
    let mut tail = if unfinished {
        lines.last_logical()
    } else {
        nlines + 1
    };
    let drop_from = tail;
    let open = lines.open();
    let mut text = format!("{}{}", src, closers(&end));
    if let Some(h) = last_code_line(&text).filter(|l| ends_block(l)) {
        text = format!("{}{}", text.trim_end_matches(' '), filler(h));
    }
    let mut mods = None;
    for _ in 0..4 {
        match module::load_with(root, Some(text.clone())) {
            Ok(m) => {
                mods = Some(m);
                break;
            }
            Err(errs) => {
                let at_tail = errs.iter().any(|e| {
                    e.file.is_empty() && matches!(e.kind, "lex" | "parse") && e.line >= tail
                });
                if !at_tail || tail <= 1 {
                    return errs
                        .into_iter()
                        .filter(|e| !e.file.is_empty() || e.line < drop_from.min(tail))
                        .collect();
                }
                // Drop the unfinished logical line; give a block header left
                // without a body a neutral one.
                let keep: Vec<&str> = src.split('\n').take(tail as usize - 1).collect();
                text = keep.join("\n");
                text.push('\n');
                if let Some(h) = last_code_line(&text).filter(|l| ends_block(l)) {
                    text = format!("{}{}", text.trim_end_matches('\n'), filler(h));
                }
                let prev = scan(&keep.join("\n")).0.last_logical();
                tail = prev.min(tail - 1);
            }
        }
    }
    let Some(mut mods) = mods else {
        return Vec::new();
    };
    // Top-level statements only on the unfinished last line (a partial
    // `test`/`struct` keyword after `fn main`) do not conflict with main yet.
    let tail_top = mods
        .first()
        .is_some_and(|m| !m.prog.top.is_empty() && m.prog.top_pos.0 >= drop_from);
    let errs = match check::check(&mut mods, need_main) {
        Ok(_) => Vec::new(),
        Err(e) => e,
    };
    std::mem::forget(mods);
    errs.into_iter()
        .filter(|e| {
            if !e.file.is_empty() {
                return true;
            }
            if e.line >= drop_from {
                return false;
            }
            let code = crate::diag::code(e.kind, &e.msg);
            if tail_top && code == "E0224" {
                return false;
            }
            // Items may be declared in any order, so a name not declared yet
            // may still come: functions, types, and (capitalized) variants.
            let later = match code {
                "E0203" | "E0204" => true,
                "E0201" => e
                    .msg
                    .split('\'')
                    .nth(1)
                    .is_some_and(|n| n.starts_with(char::is_uppercase)),
                _ => false,
            };
            if later {
                return false;
            }
            let open_line = e.line == 0 || open.get(e.line as usize - 1).copied().unwrap_or(true);
            !(OPEN_CODES.contains(&code) && (open_line || code == "E0213"))
        })
        .collect()
}

/// Token classes probed by `--expect`, with a representative spelling.
fn classes() -> Vec<(&'static str, &'static str)> {
    let mut v = vec![
        ("ident", "lim_probe_"),
        ("int", "1"),
        ("float", "1.5"),
        ("str", "\"s\""),
    ];
    for k in [
        "let", "var", "fn", "return", "if", "elif", "else", "while", "for", "in", "true", "false",
        "and", "or", "not", "break", "continue", "pass", "struct", "import", "pub", "enum",
        "match",
    ] {
        v.push((k, k));
    }
    for s in [
        "..", "==", "!=", "<=", ">=", "->", "+=", "-=", "*=", "/=", "%=", "(", ")", "{", "}", "[",
        "]", ",", ":", "+", "-", "*", "/", "%", "=", "<", ">", ".", "?", "&",
    ] {
        v.push((s, s));
    }
    v.push(("newline", "\n"));
    v
}

/// (line, col) just after `text` (col counts code points).
fn end_pos(text: &str) -> (u32, u32) {
    let line = text.matches('\n').count() as u32 + 1;
    let last = text.rsplit('\n').next().unwrap_or("");
    (line, last.chars().count() as u32 + 1)
}

/// Does the parser accept the token class spelled `tok` right after `base`?
/// It does when parsing `base + tok` fails only after that token (or not at
/// all): the parser stops at the first error.
fn accepts(base: &str, tok: &str) -> bool {
    let sep = match base.chars().last() {
        None | Some(' ' | '\n' | '(' | '[' | '{') => "",
        _ if tok == "\n" => "",
        _ => " ",
    };
    let text = format!("{}{}{}", base, sep, tok);
    let at = end_pos(&format!("{}{}", base, sep));
    let r = lexer::lex(&text).and_then(parser::parse);
    match r {
        Ok(_) => true,
        Err(e) => (e.line, e.col) > at,
    }
}

/// Start of the last top-level item of `base` (a line at indent 0 outside
/// brackets that does not continue an `if`): syntax before it cannot change
/// what may follow, so probes parse only from there.
fn item_start(base: &str) -> usize {
    let (lines, _) = scan(base);
    let mut off = 0;
    let mut best = 0;
    for (i, l) in base.split('\n').enumerate() {
        let t = l.trim_start();
        if i + 1 < lines.indent.len()
            && lines.indent[i] == Some(0)
            && lines.depth[i] == 0
            && !t.starts_with("elif")
            && !t.starts_with("else")
        {
            best = off;
        }
        off += l.len() + 1;
    }
    best
}

/// Lines above which `--expect` probes a skeleton of the current item.
const SKELETON_MIN_LINES: usize = 64;

/// A smaller text with the same syntax at its end as `cut` (one top-level
/// item): the current logical line, the headers of the blocks still open
/// there (each with the last statement of its block, which `open()` marks
/// too), and an `if`/`elif` chain before an open `elif`/`else`, with neutral
/// bodies. Earlier complete statements cannot change what may follow, and
/// dropping them makes each probe independent of the item's size.
fn skeleton(cut: &str) -> String {
    let src: Vec<&str> = cut.split('\n').collect();
    if src.len() < SKELETON_MIN_LINES {
        return cut.to_string();
    }
    let (lines, _) = scan(cut);
    // Start of the logical line holding the end of the text.
    let mut cur = src.len() - 1;
    while cur > 0 && lines.depth[cur] > 0 {
        cur -= 1;
    }
    // An error before the current line already decides every probe (none
    // is accepted): keep the text as it is.
    if let Err(e) = lexer::lex(cut).and_then(parser::parse) {
        if (e.line as usize) <= cur {
            return cut.to_string();
        }
    }
    let open = lines.open();
    let stmt = |i: usize| lines.depth[i] == 0 && lines.indent[i].is_some();
    // A logical line from `i`: it and its bracket continuation lines.
    let logical = |i: usize| {
        let mut j = i + 1;
        while j < cur && lines.depth[j] > 0 {
            j += 1;
        }
        src[i..j].to_vec()
    };
    // The `if`/`elif` chain before an `elif`/`else` at line `i`, each header
    // with a neutral body.
    let chain = |i: usize| {
        let mut out: Vec<(usize, Vec<String>)> = Vec::new();
        let t = src[i].trim_start();
        if !(t.starts_with("elif") || t.starts_with("else")) {
            return out;
        }
        let k = lines.indent[i].unwrap_or(0);
        let mut j = i;
        while j > 0 {
            j -= 1;
            if !stmt(j) || lines.indent[j].unwrap_or(0) > k {
                continue;
            }
            if lines.indent[j] != Some(k) {
                break;
            }
            let head = logical(j);
            let last = head.last().copied().unwrap_or("");
            let mut h: Vec<String> = head.iter().map(|l| l.to_string()).collect();
            h.push(filler(last).trim_matches('\n').to_string());
            out.push((j, h));
            let tj = src[j].trim_start();
            if !(tj.starts_with("elif") || tj.starts_with("else")) {
                break;
            }
        }
        out
    };
    let mut keep: Vec<(usize, Vec<String>)> = chain(cur);
    for (i, &is_open) in open.iter().enumerate().take(cur) {
        if is_open && stmt(i) {
            keep.extend(chain(i));
            keep.push((i, logical(i).iter().map(|l| l.to_string()).collect()));
        }
    }
    keep.sort_by_key(|x| x.0);
    keep.dedup_by_key(|x| x.0);
    let mut out: Vec<String> = keep.into_iter().flat_map(|x| x.1).collect();
    out.extend(src[cur..].iter().map(|l| l.to_string()));
    out.join("\n")
}

fn ty_fits(t: &Ty, want: Option<&Ty>) -> bool {
    match want {
        None => true,
        Some(w) => *t != Ty::Err && check::compat(t, w),
    }
}

fn fn_ty(ps: &[Ty], r: &Ty) -> String {
    let ps: Vec<String> = ps.iter().map(|p| p.to_string()).collect();
    match r {
        Ty::Unit => format!("fn({})", ps.join(", ")),
        r => format!("fn({}) -> {}", ps.join(", "), r),
    }
}

/// Builtins with a fixed result type: name, type shown, result.
fn builtins() -> Vec<(&'static str, &'static str, Ty)> {
    let opt = |t: Ty| Ty::Option(Box::new(t));
    vec![
        ("len", "fn([T] | str) -> int", Ty::Int),
        ("str", "fn(T) -> str", Ty::Str),
        ("int", "fn(float | str) -> int", Ty::Int),
        ("float", "fn(int | str) -> float", Ty::Float),
        ("sqrt", "fn(float) -> float", Ty::Float),
        (
            "split",
            "fn(str, str) -> [str]",
            Ty::List(Box::new(Ty::Str)),
        ),
        ("join", "fn([str], str) -> str", Ty::Str),
        ("parse_int", "fn(str) -> Option[int]", opt(Ty::Int)),
        ("parse_float", "fn(str) -> Option[float]", opt(Ty::Float)),
        ("find", "fn(str, str) -> Option[int]", opt(Ty::Int)),
        ("is_some", "fn(Option[T]) -> bool", Ty::Bool),
        ("is_none", "fn(Option[T]) -> bool", Ty::Bool),
        ("is_ok", "fn(Result[T, E]) -> bool", Ty::Bool),
        ("is_err", "fn(Result[T, E]) -> bool", Ty::Bool),
        ("any", "fn([bool]) -> bool", Ty::Bool),
        ("all", "fn([bool]) -> bool", Ty::Bool),
    ]
}

/// The names that fit at the hole: (name, type, kind).
fn names(h: &Hole, prefix: &str) -> Vec<(String, String, &'static str)> {
    if h.none_fits {
        return Vec::new();
    }
    let want = h.expected.as_ref();
    let mut out: Vec<(String, String, &'static str)> = Vec::new();
    let mut add = |n: &str, t: String, k: &'static str| {
        if n.starts_with(prefix) && !n.starts_with("lim_probe_") && n != HOLE {
            out.push((n.to_string(), t, k));
        }
    };
    let variants = |en: &str,
                    vs: &check::Variants,
                    ty: &Ty,
                    add: &mut dyn FnMut(&str, String, &'static str)| {
        for (v, fs) in vs {
            let t = if fs.is_empty() {
                ty.to_string()
            } else {
                let ps: Vec<Ty> = fs.iter().map(|f| f.1.clone()).collect();
                fn_ty(&ps, ty)
            };
            let _ = en;
            add(v, t, "variant");
        }
    };
    if let Some(en) = &h.enum_recv {
        if let Some((_, vs)) = h.enums.iter().find(|e| &e.0 == en) {
            variants(en, vs, &Ty::Enum(en.clone()), &mut add);
        }
        return out;
    }
    if let Some(r) = &h.recv {
        match r {
            Ty::Struct(s) => {
                let short = s.rsplit('.').next().unwrap_or(s);
                if let Some((_, fs)) = h.structs.iter().find(|x| x.0 == *s || x.0 == short) {
                    for (f, t) in fs {
                        if ty_fits(t, want) {
                            add(f, t.to_string(), "field");
                        }
                    }
                }
            }
            Ty::Cap(Cap::Env) => {
                for f in ["fs", "vars", "clock", "args", "stdin"] {
                    let t = Ty::Cap(Cap::env_field(f).unwrap_or(Cap::Fs));
                    if ty_fits(&t, want) {
                        add(f, t.to_string(), "field");
                    }
                }
            }
            Ty::Cap(c) => {
                for m in check::cap_methods(*c) {
                    if let Some((_, ps, r)) = check::cap_method(*c, m) {
                        if ty_fits(&r, want) {
                            add(m, fn_ty(&ps, &r), "method");
                        }
                    }
                }
            }
            _ => {}
        }
        return out;
    }
    for (n, t) in &h.vars {
        if ty_fits(t, want) {
            add(n, t.to_string(), "var");
        }
    }
    for (n, ps, r) in &h.funcs {
        if ty_fits(r, want) {
            add(n, fn_ty(ps, r), "fn");
        }
    }
    for (n, t, r) in builtins() {
        if ty_fits(&r, want) {
            add(n, t.to_string(), "builtin");
        }
    }
    match want {
        Some(Ty::Option(t)) => {
            add("Some", format!("fn({}) -> Option[{}]", t, t), "variant");
            add("None", format!("Option[{}]", t), "variant");
        }
        Some(Ty::Result(t, e)) => {
            let r = format!("Result[{}, {}]", t, e);
            add("Ok", format!("fn({}) -> {}", t, r), "variant");
            add("Err", format!("fn({}) -> {}", e, r), "variant");
        }
        Some(Ty::Bool) => {
            add("true", "bool".into(), "literal");
            add("false", "bool".into(), "literal");
        }
        _ => {}
    }
    for (en, vs) in &h.enums {
        let ty = Ty::Enum(en.clone());
        let q_match = match want {
            None => true,
            Some(Ty::Enum(q)) => q == en || q.rsplit('.').next() == Some(en.as_str()),
            Some(_) => false,
        };
        if q_match {
            variants(en, vs, &ty, &mut add);
        }
    }
    if want.is_none() {
        for (n, t) in [
            ("print", "fn(...)"),
            ("push", "fn([T], T)"),
            ("pop", "fn([T]) -> T"),
            ("sort", "fn([T])"),
            ("assert", "fn(bool, str)"),
            ("get", "fn([T], int) -> Option[T]"),
            ("unwrap", "fn(Option[T] | Result[T, E]) -> T"),
            ("unwrap_or", "fn(Option[T] | Result[T, E], T) -> T"),
            ("sum", "fn([int] | [float]) -> int | float"),
            ("min", "fn(T, T) -> T"),
            ("max", "fn(T, T) -> T"),
        ] {
            add(n, t.to_string(), "builtin");
        }
        for (s, _) in &h.structs {
            add(s, "struct".into(), "struct");
        }
    }
    out.truncate(200);
    out
}

/// At a field name of an unfinished struct literal (`P { ` or `P { a: 1, `)?
/// Returns the struct's name and the fields already given.
fn lit_field(base: &str) -> Option<(String, Vec<String>)> {
    // Innermost open bracket (outside strings and comments).
    let mut stack: Vec<(usize, char)> = Vec::new();
    let (mut in_str, mut esc, mut comment) = (false, false, false);
    for (i, ch) in base.char_indices() {
        if ch == '\n' {
            (in_str, comment) = (false, false);
            continue;
        }
        if comment {
            continue;
        }
        if in_str {
            match ch {
                _ if esc => esc = false,
                '\\' => esc = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '#' => comment = true,
            '(' | '[' | '{' => stack.push((i, ch)),
            ')' | ']' | '}' => {
                stack.pop();
            }
            _ => {}
        }
    }
    let &(at, '{') = stack.last()? else {
        return None;
    };
    let body = &base[at + 1..];
    if !(body.trim().is_empty() || body.trim_end().ends_with(',')) {
        return None;
    }
    let head = base[..at].trim_end();
    let n = head
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
        .count();
    let name = &head[head.len() - n..];
    if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    // Fields already given: `name:` at the literal's own nesting level.
    let mut used = Vec::new();
    let (mut depth, mut word, mut prev) = (0usize, String::new(), ' ');
    for ch in body.chars() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ':' if depth == 0 && !word.is_empty() => used.push(word.clone()),
            _ => {}
        }
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if prev.is_whitespace() {
                word.clear();
            }
            word.push(ch);
        } else if !ch.is_whitespace() {
            word.clear();
        }
        prev = ch;
    }
    Some((name.to_string(), used))
}

/// `--expect`: one JSON object describing what may come next at the end of
/// `src`.
pub fn expect(root: &Path, src: &str, need_main: bool) -> String {
    let (_, end) = scan(src);
    let (line, col);
    let mut fields: Vec<String> = Vec::new();
    if end.in_comment || end.in_str {
        (line, col) = end_pos(src);
        let toks = if end.in_str {
            "\"str_text\",\"str_end\""
        } else {
            "\"newline\""
        };
        let ctx = if end.in_str { "string" } else { "comment" };
        return format!(
            "{{\"kind\":\"expect\",\"line\":{},\"col\":{},\"prefix\":\"\",\"context\":\"{}\",\"tokens\":[{}],\"expected\":null,\"names\":[]}}",
            line, col, ctx, toks
        );
    }
    // A partly typed name at the cursor: complete from its start.
    let word_len = src
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .count();
    let word = &src[src.len() - word_len..];
    let (base, prefix) = if word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        (&src[..src.len() - word_len], word)
    } else {
        (src, "")
    };
    (line, col) = end_pos(base);
    let cut = skeleton(&base[item_start(base)..]);
    let cut = cut.as_str();
    let mut toks: Vec<&str> = Vec::new();
    let in_brackets = !scan(base).1.brackets.is_empty();
    for (name, spell) in classes() {
        if name == "newline" && in_brackets {
            continue;
        }
        if accepts(cut, spell) {
            toks.push(name);
        }
    }
    // Indentation levels at which a new statement may start.
    let at_line_start = base
        .rsplit('\n')
        .next()
        .is_some_and(|l| l.trim().is_empty())
        && base.contains('\n');
    if at_line_start && prefix.is_empty() {
        let head = &cut[..cut.len() - cut.rsplit('\n').next().unwrap_or("").len()];
        let lvls: Vec<String> = (0..12)
            .map(|k| k * 4)
            .filter(|&n| accepts(&format!("{}{}", head, " ".repeat(n)), "pass"))
            .map(|n| n.to_string())
            .collect();
        fields.push(format!("\"indents\":[{}]", lvls.join(",")));
    }
    let member = base.trim_end().ends_with('.');
    let mut hole: Option<Hole> = None;
    if toks.contains(&"ident") {
        let sep = match base.chars().last() {
            None | Some(' ' | '\n' | '(' | '[' | '{' | '.') => "",
            _ => " ",
        };
        let (_, bend) = scan(base);
        let close = closers(&bend);
        let cur_line = base.rsplit('\n').next().unwrap_or("");
        let ind = " ".repeat(cur_line.len() - cur_line.trim_start().len() + 4);
        'outer: for inline in ["", ": 0"] {
            for block in [
                String::new(),
                format!(":\n{}pass", ind),
                format!(":\n{}_: pass", ind),
            ] {
                let text = format!("{}{}{}{}{}{}\n", base, sep, HOLE, inline, close, block);
                if lexer::lex(&text).and_then(parser::parse).is_err() {
                    continue;
                }
                if let Ok(mut mods) = module::load_with(root, Some(text)) {
                    let (_, h) = check::check_with(&mut mods, need_main, Some(Hole::default()));
                    std::mem::forget(mods);
                    hole = h.filter(|h| h.seen);
                }
                break 'outer;
            }
        }
    }
    // A field name of a struct literal: the struct's remaining fields.
    let mut field_names: Option<Vec<String>> = None;
    if let Some((st, used)) = lit_field(base).filter(|_| toks.contains(&"ident")) {
        let sep = if base.ends_with([' ', '\n', '{']) {
            ""
        } else {
            " "
        };
        let close = closers(&scan(base).1);
        let text = format!("{}{}lim_probe_: {}{}\n", base, sep, HOLE, close);
        if let Ok(mut mods) = module::load_with(root, Some(text)) {
            let (_, h) = check::check_with(&mut mods, need_main, Some(Hole::default()));
            std::mem::forget(mods);
            let short = st.rsplit('.').next().unwrap_or(&st);
            let fs = h.and_then(|h| {
                h.structs
                    .into_iter()
                    .find(|x| x.0 == st || x.0 == short)
                    .map(|x| x.1)
            });
            field_names = fs.map(|fs| {
                fs.into_iter()
                    .filter(|(f, _)| f.starts_with(prefix) && !used.contains(f))
                    .map(|(f, t)| {
                        format!(
                            "{{\"name\":{},\"type\":{},\"kind\":\"field\"}}",
                            json_str(&f),
                            json_str(&t.to_string())
                        )
                    })
                    .collect()
            });
        }
    }
    if let Some(fs) = field_names {
        let toks: Vec<String> = toks.iter().map(|t| json_str(t)).collect();
        return format!(
            "{{\"kind\":\"expect\",\"line\":{},\"col\":{},\"prefix\":{},\"context\":\"field\",\"tokens\":[{}],\"expected\":null,\"names\":[{}]}}",
            line,
            col,
            json_str(prefix),
            toks.join(","),
            fs.join(",")
        );
    }
    let context = match (&hole, member) {
        (Some(_), true) => "member",
        (Some(_), false) if at_line_start => "statement",
        (Some(_), false) => "expression",
        (None, _) => "syntax",
    };
    let expected = hole
        .as_ref()
        .filter(|h| !h.none_fits)
        .and_then(|h| h.expected.as_ref())
        .map_or("null".to_string(), |t| json_str(&t.to_string()));
    let ns: Vec<String> = hole.as_ref().map_or(Vec::new(), |h| {
        names(h, prefix)
            .into_iter()
            .map(|(n, t, k)| {
                format!(
                    "{{\"name\":{},\"type\":{},\"kind\":\"{}\"}}",
                    json_str(&n),
                    json_str(&t),
                    k
                )
            })
            .collect()
    });
    let toks: Vec<String> = toks.iter().map(|t| json_str(t)).collect();
    let extra = if fields.is_empty() {
        String::new()
    } else {
        format!(",{}", fields.join(","))
    };
    format!(
        "{{\"kind\":\"expect\",\"line\":{},\"col\":{},\"prefix\":{},\"context\":\"{}\",\"tokens\":[{}]{},\"expected\":{},\"names\":[{}]}}",
        line,
        col,
        json_str(prefix),
        context,
        toks.join(","),
        extra,
        expected,
        ns.join(",")
    )
}
