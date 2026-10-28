//! `check --stdin --partial --expect`, rich `--json` diagnostics, `limits fix`
//! and runtime error data.

mod common;

use std::io::Write;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

fn tmp() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "limits-assist-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `limits check --stdin <flags>` with `src` on stdin: (stdout, code).
fn stdin_check(src: &str, flags: &[&str]) -> (String, i32) {
    let mut c = common::limits()
        .args(["check", "--stdin"])
        .args(flags)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(src.as_bytes()).unwrap();
    let o = c.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}

fn expect_line(src: &str) -> String {
    let (out, _) = stdin_check(src, &["--expect", "--json"]);
    out.lines()
        .find(|l| l.starts_with("{\"kind\":\"expect\""))
        .unwrap_or_else(|| panic!("no expect object for {:?}: {}", src, out))
        .to_string()
}

const PROGRAM: &str = r#"enum Shape:
    Circle(r: float)
    Rect(w: float, h: float)
struct Item:
    name: str
    qty: int
fn area(s: Shape) -> float:
    match s:
        Circle(r): 3.0 * r * r
        Rect(w, h): w * h
fn total(items: [Item]) -> int:
    var t = 0
    for it in items:
        t += it.qty
    t
let xs = [Item { name: "a", qty: 2 }, Item { name: "b", qty: 3 }]
print(total(xs), area(Rect(2.0, 3.0)), helper("x"))
fn helper(s: str) -> str: s + "!"
"#;

#[test]
fn prefixes_of_a_valid_program_have_no_errors() {
    let (out, code) = stdin_check(PROGRAM, &["--json"]);
    assert_eq!((out.as_str(), code), ("", 0));
    // Every prefix (at every char) is a valid unfinished program.
    let cuts: Vec<usize> = (0..=PROGRAM.len()).step_by(3).collect();
    for k in cuts {
        let (out, code) = stdin_check(&PROGRAM[..k], &["--partial", "--json"]);
        assert_eq!(
            (out.as_str(), code),
            ("", 0),
            "prefix of {} bytes:\n{}",
            k,
            &PROGRAM[..k]
        );
    }
}

#[test]
fn partial_reports_real_errors_in_the_complete_part() {
    let src = "let x: int = \"s\"\nprint(totl)\nlet y = foo(1,\n    2";
    let (out, code) = stdin_check(src, &["--partial", "--json"]);
    assert_eq!(code, 1);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{}", out);
    assert!(lines[0].contains("\"code\":\"E0301\""), "{}", out);
    assert!(
        lines[0].contains("\"expected\":\"int\",\"found\":\"str\""),
        "{}",
        out
    );
    assert!(lines[1].contains("\"code\":\"E0201\""), "{}", out);
    // A syntax error in a finished line is real; an unfinished one is not.
    let (out, code) = stdin_check("let a = 1 +\nlet b = (", &["--partial", "--json"]);
    assert_eq!(code, 1);
    assert!(
        out.contains("\"line\":1,") && !out.contains("\"line\":2,"),
        "{}",
        out
    );
    // Unfinished blocks: no missing return / arms / main.
    let (out, code) = stdin_check(
        "fn f(n: int) -> int:\n    let m = n\n    match Some(m):\n        Some(v): ",
        &["--partial", "--json"],
    );
    assert_eq!((out.as_str(), code), ("", 0));
    // Without --partial the same input is an error.
    let (_, code) = stdin_check("fn f(n: int) -> int:\n", &["--json"]);
    assert_eq!(code, 1);
}

#[test]
fn expect_gives_tokens_type_and_fitting_names() {
    let e = expect_line("fn f(a: int, s: str) -> int:\n    if a > 0:\n        return ");
    assert!(e.contains("\"context\":\"expression\""), "{}", e);
    assert!(e.contains("\"expected\":\"int\""), "{}", e);
    assert!(
        e.contains("{\"name\":\"a\",\"type\":\"int\",\"kind\":\"var\"}"),
        "{}",
        e
    );
    assert!(
        e.contains("{\"name\":\"f\",\"type\":\"fn(int, str) -> int\",\"kind\":\"fn\"}"),
        "{}",
        e
    );
    assert!(!e.contains("\"name\":\"s\""), "{}", e);
    assert!(e.contains("\"tokens\":[\"ident\",\"int\""), "{}", e);
    assert!(!e.contains("\"let\""), "{}", e);
    // Fields after '.', filtered by the expected type.
    let e = expect_line(
        "struct P:\n    name: str\n    x: int\nfn f(p: P) -> int:\n    let y: int = p.",
    );
    assert!(e.contains("\"context\":\"member\""), "{}", e);
    assert!(
        e.contains("\"names\":[{\"name\":\"x\",\"type\":\"int\",\"kind\":\"field\"}]"),
        "{}",
        e
    );
    // Enum variants for an enum-typed hole; a partial name filters.
    let e = expect_line("enum Color:\n    Red\n    Green\nfn f() -> Color:\n    let c: Color = Gr");
    assert!(e.contains("\"prefix\":\"Gr\""), "{}", e);
    assert!(
        e.contains("\"names\":[{\"name\":\"Green\",\"type\":\"Color\",\"kind\":\"variant\"}]"),
        "{}",
        e
    );
    let e = expect_line("fn f(n: int) -> Option[int]:\n    if n > 0: return So");
    assert!(e.contains("\"expected\":\"Option[int]\""), "{}", e);
    assert!(e.contains("\"name\":\"Some\""), "{}", e);
    // Statement start: keywords, indentation levels.
    let e = expect_line("let xs = [1]\nfor x in xs:\n    print(x)\n");
    assert!(e.contains("\"context\":\"statement\""), "{}", e);
    assert!(e.contains("\"indents\":[0,4]"), "{}", e);
    assert!(e.contains("\"for\""), "{}", e);
    // After a complete expression inside brackets: operators, no newline.
    let e = expect_line("let y = max(1, 2");
    assert!(
        e.contains("\")\"") && e.contains("\",\"") && !e.contains("newline"),
        "{}",
        e
    );
    // Inside a string.
    let e = expect_line("let s = \"ab");
    assert!(e.contains("\"context\":\"string\""), "{}", e);
}

#[test]
fn json_diagnostics_have_spans_alternatives_and_fixes() {
    let src = "enum Shape:\n    Circle(r: float)\n    Dot\nfn show(s: Shape):\n    match s:\n        Circle(r): print(r)\nlet total = 3\ntotal = 4\nfn bump(xs: inout [int]): push(xs, 1)\nvar a = [1]\nbump(a)\nprint(totl)\nprint(env.clock.now_ms())\n";
    let (out, code) = stdin_check(src, &["--json"]);
    assert_eq!(code, 1);
    let ls: Vec<&str> = out.lines().collect();
    assert_eq!(ls.len(), 5, "{}", out);
    assert!(ls[0].contains(r#""alternatives":["Dot"],"fixes":[{"msg":"add the missing arm(s): Dot","edits":[{"line":6,"col":1,"end_line":6,"end_col":1,"text":"        Dot: pass\n"}],"applicability":"machine"}]"#), "{}", ls[0]);
    assert!(ls[1].contains(r#""source_line":"total = 4""#), "{}", ls[1]);
    assert!(ls[1].contains(r#"{"msg":"declare 'total' with 'var'","edits":[{"line":7,"col":1,"end_line":7,"end_col":4,"text":"var"}],"applicability":"machine"}"#), "{}", ls[1]);
    assert!(
        ls[2].contains(r#""edits":[{"line":11,"col":6,"end_line":11,"end_col":6,"text":"&"}]"#),
        "{}",
        ls[2]
    );
    assert!(ls[3].contains(r#""end_line":12,"end_col":11"#), "{}", ls[3]);
    assert!(
        ls[3].contains(r#""text":"total"}],"applicability":"machine""#),
        "{}",
        ls[3]
    );
    assert!(ls[4].contains(r#""text":"uses time\n""#), "{}", ls[4]);
    let (out, _) = stdin_check("let x = 1 + 2.0\n", &["--json"]);
    assert!(
        out.contains(r#""expected":"int, float or str on both sides","found":"int and float""#),
        "{}",
        out
    );
    let (out, _) = stdin_check("print(len(1, 2))\n", &["--json"]);
    assert!(
        out.contains(r#""expected":"1 argument(s)","found":"2 argument(s)""#),
        "{}",
        out
    );
    let (out, _) = stdin_check("let x = (1\n", &["--json"]);
    assert!(
        out.contains(r#""expected":"')'","found":"end of line""#),
        "{}",
        out
    );
}

#[test]
fn fix_applies_machine_fixes_and_rechecks() {
    let d = tmp();
    let f = d.join("t.lim");
    let src = "uses args\nenum E:\n    A\n    B\nfn show(e: E):\n    match e:\n        A: print(\"a\")\nlet total = 3\ntotal = 4\nfn bump(xs: inout [int]): push(xs, 1)\nvar a = [1]\nbump(a)\nshow(B)\nprint(totl, a, len(env.args.list()), env.clock.now_ms() > 0)\n";
    std::fs::write(&f, src).unwrap();
    let o = common::limits()
        .args(["fix", f.to_str().unwrap()])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(0), "{}", out);
    assert!(
        out.ends_with("5 fix(es) applied, 0 error(s) remain\n"),
        "{}",
        out
    );
    assert!(out.contains("fixed "), "{}", out);
    let fixed = std::fs::read_to_string(&f).unwrap();
    assert!(fixed.starts_with("uses args, time\n"), "{}", fixed);
    assert!(
        fixed.contains("        B: pass\n") && fixed.contains("var total = 3\n"),
        "{}",
        fixed
    );
    assert!(
        fixed.contains("bump(&a)\n") && fixed.contains("print(total, a"),
        "{}",
        fixed
    );
    let o = common::limits()
        .args(["run", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout), "4 [1, 1] 0 true\n");
    // Nothing machine-fixable: the errors remain and the file is unchanged.
    std::fs::write(&f, "let x = 1 + 2.0\n").unwrap();
    let o = common::limits()
        .args(["fix", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stdout).ends_with("0 fix(es) applied, 1 error(s) remain\n"));
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "let x = 1 + 2.0\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn runtime_json_errors_carry_observed_values() {
    let d = tmp();
    for (src, flags, data) in [
        (
            "let xs = [1, 2]\nprint(xs[5])\n",
            &[][..],
            r#""data":{"index":5,"len":2}"#,
        ),
        (
            "var i = 0\nwhile true: i += 1\n",
            &["--max-steps", "500"][..],
            r#""data":{"limit":500}"#,
        ),
        (
            "print(9223372036854775807 + 1)\n",
            &[][..],
            r#""data":{"a":9223372036854775807,"b":1}"#,
        ),
        (
            "uses fs.read\nprint(env.fs.read(\"nope.txt\"))\n",
            &[][..],
            r#""data":{"path":"nope.txt"}"#,
        ),
    ] {
        std::fs::write(d.join("r.lim"), src).unwrap();
        let o = common::limits()
            .args(["run", "r.lim", "--json"])
            .args(flags)
            .current_dir(&d)
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(
            err.trim_end().ends_with(&format!("{}}}", data)),
            "{}: {}",
            src,
            err
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// First JSON diagnostic of `check --stdin --json` (or --partial): (line,
/// col, code) as text fragments.
fn first_diag(src: &str, flags: &[&str]) -> (String, i32) {
    let mut f = vec!["--json"];
    f.extend(flags);
    let (out, code) = stdin_check(src, &f);
    (out.lines().next().unwrap_or("").to_string(), code)
}

/// Reviewer regression: an unfinished last line after `fn main` (the start
/// of `test`, `struct`, ...) parsed as a top-level statement and gave E0224.
#[test]
fn partial_keyword_after_main_is_not_a_conflict() {
    for p in ["t", "te", "tes", "test", "test ", "struct", "enu"] {
        let src = format!("fn main():\n    pass\n{}", p);
        let (out, code) = first_diag(&src, &["--partial"]);
        assert_eq!((out.as_str(), code), ("", 0), "{:?}", p);
    }
    // A complete top-level statement still conflicts.
    let (out, code) = first_diag("fn main():\n    pass\nprint(1)\nx", &["--partial"]);
    assert_eq!(code, 1);
    assert!(out.contains("\"E0224\""), "{}", out);
}

/// Reviewer regression: no `ident` (and no field names) at the first field
/// of a struct literal.
#[test]
fn expect_offers_struct_literal_fields() {
    let pre = "struct P:\n    alpha: int\n    beta: str\nlet p = ";
    for (tail, names) in [
        ("P { ", vec!["alpha", "beta"]),
        ("P { al", vec!["alpha"]),
        ("P {\n    ", vec!["alpha", "beta"]),
        ("P { alpha: 1, ", vec!["beta"]),
    ] {
        let x = expect_line(&format!("{}{}", pre, tail));
        assert!(x.contains("\"tokens\":[\"ident\",\"}\"]"), "{}", x);
        assert!(x.contains("\"context\":\"field\""), "{}", x);
        for n in ["alpha", "beta"] {
            let has = x.contains(&format!("\"name\":\"{}\"", n));
            assert_eq!(has, names.contains(&n), "{:?} {}: {}", tail, n, x);
        }
    }
}

/// Reviewer regression: diagnostics about a member/index/binary expression
/// pointed at its operator token instead of the expression.
#[test]
fn spans_start_at_the_expression() {
    for (src, line, col) in [
        ("struct P:\n    a: int\nlet p = P { a: 1 }\np.a = 2\n", 4, 1),
        ("let xs = [1]\nxs[0] = 2\n", 2, 1),
        ("let x: float = 1 + 2\n", 1, 16),
        ("let xs = [1]\nlet y: str = xs[0]\n", 2, 14),
        ("let s = \"abc\"\nlet n: int = s.len\n", 2, 14),
        ("fn f(a: int):\n    pass\nf(1.5 * 2.0)\n", 3, 3),
    ] {
        let (out, _) = first_diag(src, &[]);
        let want = format!("\"line\":{},\"col\":{},", line, col);
        assert!(out.contains(&want), "{:?}: {}", src, out);
    }
}

/// Reviewer regression: non-UTF-8 stdin printed plain text (exit 2) instead
/// of the JSON E0005 diagnostic a file gets.
#[test]
fn non_utf8_stdin_is_a_json_diagnostic() {
    for flags in [&[][..], &["--expect"]] {
        let mut c = common::limits()
            .args(["check", "--stdin", "--json"])
            .args(flags)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(b"print(\xff)\n").unwrap();
        let o = c.wait_with_output().unwrap();
        let out = String::from_utf8_lossy(&o.stdout);
        assert_eq!(o.status.code(), Some(1), "{}", out);
        assert!(
            out.starts_with(
                "{\"kind\":\"lex\",\"code\":\"E0005\",\"file\":\"<stdin>\",\"line\":1,\"col\":7"
            ),
            "{}",
            out
        );
    }
}

/// Reviewer regression: after an operator that does not apply to the left
/// operand's type, --expect offered names that are type errors.
#[test]
fn expect_offers_no_names_after_an_inapplicable_operator() {
    let x = expect_line("let xs = [1, 2]\nlet t: [int] = xs + ");
    assert!(x.ends_with("\"expected\":null,\"names\":[]}"), "{}", x);
    let x = expect_line("let n = 1\nlet t = n + ");
    assert!(
        x.contains("\"expected\":\"int\"") && x.contains("\"name\":\"n\""),
        "{}",
        x
    );
}

/// Reviewer regression: calling a variable said "undefined function" and
/// --partial hid it as a possible later declaration.
#[test]
fn calling_a_variable_is_reported_as_such() {
    for flags in [&[][..], &["--partial"]] {
        let (out, code) = first_diag("let x = 1\nlet y = x(2)\n", flags);
        assert_eq!(code, 1, "{}", out);
        assert!(out.contains("\"code\":\"E0228\""), "{}", out);
        assert!(
            out.contains("'x' is a variable of type int, not a function"),
            "{}",
            out
        );
    }
}

/// Reviewer regression: `limits fix` renamed a typo'd assignment target to a
/// `let` variable and then made that `var`: two machine fixes that silently
/// changed which variable is assigned.
#[test]
fn fix_does_not_chain_rename_into_let_to_var() {
    let d = tmp();
    let f = d.join("w.lim");
    let src = "fn main():\n    let value = 1\n    if true:\n        valu = 2\n";
    std::fs::write(&f, src).unwrap();
    let o = common::limits()
        .args(["fix", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(std::fs::read_to_string(&f).unwrap(), src);
    // The rename is still offered, for review.
    let o = common::limits()
        .args(["check", f.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("\"msg\":\"rename 'valu' to 'value'\"")
            && err.contains("\"applicability\":\"maybe\""),
        "{}",
        err
    );
    // To a `var` the rename is complete by itself and stays a machine fix.
    let src = "fn main():\n    var value = 1\n    if true:\n        valu = 2\n    print(value)\n";
    std::fs::write(&f, src).unwrap();
    let o = common::limits()
        .args(["fix", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert!(std::fs::read_to_string(&f)
        .unwrap()
        .contains("        value = 2\n"));
    let _ = std::fs::remove_dir_all(&d);
}

/// Reviewer regression: --expect cost grew quadratically with the size of the
/// current item (the hole's scope listing and the token probes). The skeleton
/// used for large items must give the same answer as the full text.
#[test]
fn expect_on_a_large_item_is_fast_and_unchanged() {
    let mut src = String::from("fn main():\n");
    for i in 0..6000 {
        src.push_str(&format!("    let v{} = {}\n", i, i));
    }
    src.push_str("    if v1 > 0:\n        pass\n    elif v2 > 0:\n        print(v3)\n    el");
    let t = std::time::Instant::now();
    let x = expect_line(&src);
    assert!(t.elapsed().as_secs_f64() < 5.0, "{:?}", t.elapsed());
    assert!(x.contains("\"prefix\":\"el\""), "{}", x);
    assert!(x.contains("\"elif\"") && x.contains("\"else\""), "{}", x);
    let mut s2 = src[..src.len() - 6].to_string();
    s2.push_str("\n    let z = v1 + ");
    let x = expect_line(&s2);
    assert!(
        x.contains("\"expected\":\"int\"") && x.contains("\"name\":\"v1\","),
        "{}",
        x
    );
}
