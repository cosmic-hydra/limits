//! Enums, Option/Result, match and `?`: end-to-end and error tests.

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

/// Run `limits <cmd> t.lim [flags]`: (stdout, stderr, exit code).
fn limits(cmd: &str, src: &str, flags: &[&str]) -> (String, String, i32) {
    let dir = std::env::temp_dir().join(format!(
        "limits-sum-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("t.lim"), src).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_limits"))
        .arg(cmd)
        .arg(dir.join("t.lim"))
        // Every program run by the tests must free all its memory.
        .args(if matches!(cmd, "run" | "test") {
            &["--leak-check"][..]
        } else {
            &[]
        })
        .args(flags)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}

fn ok(src: &str, want: &str) {
    for flags in [&[][..], &["--unchecked"][..]] {
        let (out, err, code) = limits("run", src, flags);
        assert_eq!(err, "", "stderr not empty ({:?})", flags);
        assert_eq!(code, 0);
        assert_eq!(out, want, "{:?}", flags);
    }
}

/// `limits check` fails with exactly these diagnostics.
fn check_fails(src: &str, want: &str) {
    let (_, err, code) = limits("check", src, &[]);
    assert_eq!(code, 1, "stderr: {}", err);
    assert_eq!(err.trim_end(), want);
}

const SHAPE: &str = "enum Shape:\n    Circle(r: float)\n    Rect(w: float, h: float)\n    Empty\n";

#[test]
fn enum_values_print_and_match() {
    let src = format!(
        "{}fn area(s: Shape) -> float:\n    match s:\n        Circle(r): 3.0 * r * r\n        Rect(w, h): w * h\n        Empty: 0.0\n\
         let xs = [Circle(1.0), Shape.Rect(2.0, 3.0), Empty, Shape.Empty]\n\
         print(xs, str(xs[1]))\nprint([area(s) for s in xs])\n",
        SHAPE
    );
    ok(
        &src,
        "[Circle(1.0), Rect(2.0, 3.0), Empty, Empty] Rect(2.0, 3.0)\n[3.0, 6.0, 0.0, 0.0]\n",
    );
}

#[test]
fn non_exhaustive_match_lists_missing_variants() {
    let src = format!(
        "{}fn area(s: Shape) -> float:\n    match s:\n        Circle(r): r\nprint(0)\n",
        SHAPE
    );
    check_fails(
        &src,
        "check error at 6:11: match on Shape is not exhaustive: missing Rect, Empty\n  hint: add 'Rect(w, h): ...', 'Empty: ...' (or a final '_: ...' arm)",
    );
    let (_, err, _) = limits("check", &src, &["--json"]);
    assert!(err.contains("\"code\":\"E0317\""), "{}", err);
    check_fails(
        "fn f(o: Option[int]) -> int:\n    match o:\n        Some(x): x\nprint(0)\n",
        "check error at 2:11: match on Option[int] is not exhaustive: missing None\n  hint: add 'None: ...' (or a final '_: ...' arm)",
    );
}

#[test]
fn literal_matches_need_a_wildcard() {
    check_fails(
        "match 3:\n    1: print(1)\n",
        "check error at 1:7: match on int is not exhaustive: it needs a final '_' arm\n  hint: literal patterns cannot cover every int; add '_: ...' as the last arm",
    );
    check_fails(
        "match true:\n    true: pass\n",
        "check error at 1:7: match on bool is not exhaustive: missing false\n  hint: add 'false: ...' (or a final '_: ...' arm)",
    );
    ok(
        "for s in [\"a\", \"b\", \"c\"]:\n    match s:\n        \"a\": print(1)\n        \"b\": print(2)\n        _: print(0)\nfor n in [-1, 0, 5]:\n    match n:\n        -1: print(\"neg\")\n        0: print(\"zero\")\n        _: print(\"pos\")\nmatch 1 < 2:\n    true: print(\"t\")\n    false: print(\"f\")\n",
        "1\n2\n0\nneg\nzero\npos\nt\n",
    );
}

#[test]
fn pattern_errors() {
    let src = format!(
        "{}match Circle(1.0):\n    Circle(a, b): pass\n    Square: pass\n    _: pass\n    Empty: pass\nmatch 1:\n    \"x\": pass\n    _: pass\n",
        SHAPE
    );
    check_fails(
        &src,
        "check error at 6:5: pattern 'Circle' has 2 binding(s) but the variant has 1 field(s)\n  hint: write Circle(r)\n\
         check error at 7:5: Shape has no variant 'Square'\n  hint: its variants are: Circle, Rect, Empty\n\
         check error at 9:5: unreachable match arm: an earlier '_' arm matches everything\n  hint: move the '_' arm last\n\
         check error at 11:5: pattern \"x\" is a str but the matched value is int",
    );
}

#[test]
fn nested_and_recursive_enums() {
    ok(
        "enum Expr:\n    Lit(int)\n    Add(a: Expr, b: Expr)\n    Neg(Expr)\n\
         fn eval(e: Expr) -> int:\n    match e:\n        Lit(v): v\n        Add(a, b): eval(a) + eval(b)\n        Neg(x): -eval(x)\n\
         let e = Add(Lit(2), Neg(Add(Lit(3), Lit(4))))\nprint(e, eval(e))\n\
         let n: Option[Option[int]] = Some(None)\nprint(n, Some(Some(1)))\n\
         let r: Result[Option[int], str] = Ok(None)\nprint(r)\n",
        "Add(Lit(2), Neg(Add(Lit(3), Lit(4)))) -5\nSome(None) Some(Some(1))\nOk(None)\n",
    );
}

#[test]
fn enums_in_lists_and_structs() {
    let src = format!(
        "{}struct Scene:\n    shapes: [Shape]\n    best: Option[Shape]\n    log: [Result[int, str]]\n\
         var s = Scene {{ shapes: [Empty], best: None, log: [] }}\npush(s.shapes, Circle(2.0))\ns.best = Some(s.shapes[1])\n\
         push(s.log, Ok(1))\npush(s.log, Err(\"no\"))\nprint(s)\n\
         for x in s.log:\n    match x:\n        Ok(v): print(\"ok\", v)\n        Err(m): print(\"err\", m)\n",
        SHAPE
    );
    ok(
        &src,
        "Scene {shapes: [Empty, Circle(2.0)], best: Some(Circle(2.0)), log: [Ok(1), Err(\"no\")]}\nok 1\nerr no\n",
    );
}

#[test]
fn option_and_result_builtins() {
    ok(
        "print(parse_int(\"42\"), parse_int(\"4x\"), parse_int(\"99999999999999999999\"), parse_float(\"2.5\"), parse_float(\"\"))\n\
         print(find([3, 4, 5], 5), find([\"a\"], \"b\"), find(\"héllo\", \"llo\"), find(\"abc\", \"\"), get([1, 2], 1), get([1, 2], 2), get([1], -1))\n\
         print(unwrap(parse_int(\"-7\")), unwrap_or(parse_int(\"x\"), 0), is_some(find([1], 1)), is_none(get([1], 3)))\n\
         let r: Result[int, str] = Err(\"bad\")\nprint(is_ok(r), is_err(r), unwrap_or(r, -1))\n",
        "Some(42) None None Some(2.5) None\nSome(2) None Some(2) Some(0) Some(2) None None\n-7 0 true true\nfalse true -1\n",
    );
    let (_, err, code) = limits(
        "run",
        "let r: Result[int, str] = Err(\"bad\")\nprint(unwrap(r))\n",
        &["--json"],
    );
    assert_eq!(code, 1);
    assert!(
        err.contains("\"code\":\"E0517\"") && err.contains("unwrap of Err(\\\"bad\\\")"),
        "{}",
        err
    );
    let (_, err, _) = limits("run", "print(unwrap(get([1], 4)))\n", &[]);
    assert_eq!(err.trim_end(), "runtime error at 1:7: unwrap of None");
}

#[test]
fn question_mark_returns_early() {
    ok(
        "fn half(n: int) -> Result[int, str]:\n    if n % 2 == 1:\n        return Err(\"odd \" + str(n))\n    Ok(n / 2)\n\
         fn quarter(n: int) -> Result[int, str]:\n    ensures is_ok(result) or n % 2 == 1 or n % 4 != 0\n    let h = half(n)?\n    half(h)\n\
         fn add(a: str, b: str) -> Option[int]: Some(parse_int(a)? + parse_int(b)?)\n\
         print(quarter(8), quarter(6), quarter(3))\nprint(add(\"1\", \"2\"), add(\"1\", \"x\"))\n",
        "Ok(2) Err(\"odd 3\") Err(\"odd 3\")\nSome(3) None\n",
    );
    check_fails(
        "fn f() -> int:\n    let x = parse_int(\"1\")?\n    x\nfn g() -> Result[int, int]:\n    let r: Result[int, str] = Ok(1)\n    Ok(r?)\nprint(0)\n",
        "check error at 2:27: '?' on Option[int] needs the enclosing fn to return Option[_], but it returns int\n  hint: handle the value with match or unwrap_or, or change the fn's return type\n\
         check error at 6:9: '?' on Result[int, str] needs the enclosing fn to return Result[_, str], but it returns Result[int, int]\n  hint: handle the value with match or unwrap_or, or change the fn's return type",
    );
}

#[test]
fn match_as_a_value() {
    ok(
        "fn first_neg(xs: [int]) -> int:\n    var i = 0\n    while true:\n        let v = match get(xs, i):\n            Some(v): v\n            None: return 0\n        if v < 0:\n            return v\n        i += 1\n    0\n\
         let s = match parse_int(\"21\"):\n    Some(v):\n        let w = v * 2\n        \"got \" + str(w)\n    None: \"none\"\n\
         print(s, first_neg([1, -3]), first_neg([1]))\n\
         fn name(n: int) -> str:\n    return match n:\n        1: \"one\"\n        _: \"many\"\n\
         var t = \"\"\nfor k in [1, 2]:\n    t = match k:\n        1: name(k)\n        _: t + \"/\" + name(k)\nprint(t)\n",
        "got 42 -3 0\none/many\n",
    );
    check_fails(
        "let x = match 1:\n    1: 5\n    _: print(2)\nprint(x)\n",
        "check error at 3:8: match arm value must be int, got unit",
    );
}

#[test]
fn inference_and_comparison_errors() {
    check_fails(
        "let a = None\nlet b = Ok(3)\nprint(Some(1) == Some(1))\n",
        "check error at 1:9: cannot infer the type of 'None'\n  hint: add a type annotation, e.g. 'var x: Option[int] = None'\n\
         check error at 2:9: cannot infer the type of 'Ok(...)': Result needs both its value and error types\n  hint: add a type annotation, e.g. 'let r: Result[int, str] = ...', or return it from a fn declared '-> Result[int, str]'\n\
         check error at 3:15: cannot apply '==' to Option[int] and Option[int]\n  hint: sum types cannot be compared or combined with operators; use match, or is_some/is_none/is_ok/is_err",
    );
    check_fails(
        &format!("{}let c = Rect(1.0)\nlet d: Option = None\nenum Bad:\n    Some(int)\n    Circle\n", SHAPE),
        "check error at 5:9: 'Rect' takes 2 argument(s), got 1\n  hint: declared as Rect(w: float, h: float)\n\
         check error at 6:8: 'Option' needs type arguments\n  hint: write Option[T], e.g. Option[int]\n\
         check error at 8:5: 'Some' is a builtin and cannot be redefined\n\
         check error at 9:5: variant 'Circle' defined twice (also in enum 'Shape')\n  hint: variant names share one namespace per module; rename one",
    );
}

#[test]
fn shapes_example() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/shapes.lim"))
        .unwrap();
    ok(
        &src,
        "Circle(2.0) 12.56636\nRect(2.0, 3.5) 7.0\nEmpty 0.0\nerror: unknown shape 'hexagon'\nerror: missing value 2\nerror: not a number: 'x'\n",
    );
}
