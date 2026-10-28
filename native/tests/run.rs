//! End-to-end tests: compile real .lim programs with limits and run them.

mod common;

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

struct Out {
    stdout: String,
    stderr: String,
    code: i32,
}

fn limits(cmd: &str, src: &str, flags: &[&str]) -> Out {
    let dir = std::env::temp_dir().join(format!(
        "limits-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("t.lim");
    std::fs::write(&file, src).unwrap();
    let o = common::limits()
        .arg(cmd)
        .arg(&file)
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
    Out {
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        code: o.status.code().unwrap_or(-1),
    }
}

fn run(src: &str) -> Out {
    limits("run", src, &[])
}

/// Run and expect success with exactly this stdout.
fn ok(src: &str, want: &str) {
    let o = run(src);
    assert_eq!(o.stderr, "", "stderr not empty");
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, want);
}

/// Expect a failure whose stderr is exactly `want` (plus trailing newline).
fn fails(o: Out, want: &str) {
    assert_eq!(o.code, 1, "stdout: {}\nstderr: {}", o.stdout, o.stderr);
    assert_eq!(o.stderr.trim_end(), want);
}

#[test]
fn hello_print_mixed() {
    ok(r#"print("hi", 1, -2.5, true, 3.0)"#, "hi 1 -2.5 true 3.0\n");
}

#[test]
fn fn_main_is_still_an_entry_point() {
    ok("fn main():\n    print(\"from main\")\n", "from main\n");
}

#[test]
fn arithmetic_and_precedence() {
    ok(
        "print(1 + 2 * 3, (1 + 2) * 3, 7 / 2, 7 % 3, -7 / 2, 2.0 * 0.25, 1.0 / 3.0)",
        "7 9 3 1 -3 0.5 0.3333333333333333\n",
    );
}

#[test]
fn comparisons_and_logic() {
    ok(
        r#"print(1 < 2, 2 <= 1, "a" < "b", "x" == "x", 1 != 1, true and not false, false or false)"#,
        "true false true true false true false\n",
    );
}

#[test]
fn string_concat_len_and_str() {
    ok(
        "let s = \"ab\" + \"cé\"\nprint(s, len(s), str(42) + str(1.5) + str(true))",
        "abcé 4 421.5true\n",
    );
}

#[test]
fn conversions() {
    ok(
        r#"print(int(3.9), int(-3.9), float(2), int("123"), float("2.5") + 1.0)"#,
        "3 -3 2.0 123 3.5\n",
    );
}

#[test]
fn if_else_chain() {
    let src = r#"
fn sign(x: int) -> str:
    if x < 0:
        return "neg"
    elif x == 0:
        return "zero"
    else if x == 1:
        return "one"
    else:
        return "pos"

print(sign(-5), sign(0), sign(1), sign(9))
"#;
    ok(src, "neg zero one pos\n");
}

#[test]
fn implicit_return_of_last_expression() {
    let src = r#"
fn sq(x: int) -> int: x * x
fn sign(x: int) -> str:
    if x < 0:
        "neg"
    elif x == 0:
        "zero"
    else:
        "pos"
fn fact(n: int) -> int:
    if n <= 1:
        return 1
    n * fact(n - 1)
print(sq(7), sign(-1), sign(0), sign(2), fact(5))
"#;
    ok(src, "49 neg zero pos 120\n");
}

#[test]
fn while_break_continue() {
    let src = "var i = 0\nvar s = 0\nwhile true:\n    i += 1\n    if i > 10: break\n    if i % 2 == 0:\n        continue\n    s += i\nprint(s)";
    ok(src, "25\n");
}

#[test]
fn for_range_and_list() {
    let src = "var t = 0\nfor i in 0..5: t += i\nlet xs = [10, 20, 30]\nfor v in xs:\n    t = t + v\nfor i in 3..1:\n    t = 0\nprint(t)";
    ok(src, "70\n");
}

#[test]
fn loop_variable_is_immutable() {
    fails(
        limits("check", "for i in 0..3:\n    i = 100\n", &[]),
        "check error at 2:5: cannot assign to loop variable 'i'\n  hint: copy it first: var i2 = i",
    );
}

#[test]
fn let_is_immutable_var_is_mutable() {
    fails(
        limits(
            "check",
            "let x = 1\nx = 2\nx += 1\nfn f(n: int) -> int:\n    n -= 1\n    n\n",
            &[],
        ),
        "check error at 2:1: cannot assign to 'x': it is declared with 'let'\n  \
         hint: declare it with 'var x = ...' to allow reassignment\n\
         check error at 3:1: cannot assign to 'x': it is declared with 'let'\n  \
         hint: declare it with 'var x = ...' to allow reassignment\n\
         check error at 5:5: cannot assign to parameter 'n'\n  \
         hint: parameters are read-only; copy it first: var n = n (or declare 'n: inout T' and pass '&n' to change the caller's variable)",
    );
    // A `let` value cannot change at all, not even its contents.
    fails(
        limits("check", "let xs = [1]\nxs[0] = 5\npush(xs, 2)\n", &[]),
        "check error at 2:1: cannot change 'xs': it is declared with 'let'\n  \
         hint: declare it with 'var xs = ...' to allow changes\n\
         check error at 3:6: cannot change 'xs': it is declared with 'let'\n  \
         hint: declare it with 'var xs = ...' to allow changes",
    );
    ok(
        "var xs = [1]\nxs[0] = 5\npush(xs, 2)\nfn f(n: int) -> int:\n    var n = n\n    n -= 1\n    n\nprint(xs, f(3))",
        "[5, 2] 2\n",
    );
}

#[test]
fn compound_assignment() {
    let src = r#"
struct P:
    x: int
    f: float
var i = 10
i += 5
i -= 3
i *= 4
i /= 3
i %= 7
var s = "a"
s += "b" + s
var xs = [1, 2, 3]
xs[1] += 40
xs[2] *= xs[2]
var p = P { x: 1, f: 0.5 }
p.x += 10
p.f *= 3.0
print(i, s, xs, p)
"#;
    ok(src, "2 aba [1, 42, 9] P {x: 11, f: 1.5}\n");
    fails(
        limits("check", "var x = 1\nx += 1.5\nvar s = \"a\"\ns -= \"b\"\n", &[]),
        "check error at 2:3: cannot apply '+=' to int and float (use float(x) or int(x) to convert)\n\
         check error at 4:3: cannot apply '-=' to str and str",
    );
    fails(
        run("var x = 9223372036854775807\nx += 1\n"),
        "runtime error at 2:3: integer overflow in '+'",
    );
    // The target's list and index are evaluated once.
    let once = r#"
fn idx(log: inout [int]) -> int:
    push(log, 1)
    0
var log: [int] = []
var xs = [5]
xs[idx(&log)] += 1
print(xs, len(log))
"#;
    ok(once, "[6] 1\n");
}

#[test]
fn lists_push_pop_index_assign() {
    ok(
        "var xs: [int] = []\npush(xs, 1)\npush(xs, 2)\npush(xs, 3)\nxs[0] = 9\nlet p = pop(xs)\nprint(xs, p, len(xs))",
        "[9, 2] 3 2\n",
    );
}

#[test]
fn nested_lists_and_strings_print() {
    ok(
        r#"let m = [[1, 2], [3]]
let e: [[str]] = [[], ["a\"b"]]
print(m, e, m[0][1])"#,
        "[[1, 2], [3]] [[], [\"a\\\"b\"]] 2\n",
    );
}

#[test]
fn multi_line_expressions_inside_brackets() {
    let src = "let xs = [\n    1,\n  2,\n        3,\n]\nprint(len(xs),\n      xs)\n";
    ok(src, "3 [1, 2, 3]\n");
}

#[test]
fn comprehensions_and_ranges() {
    let src = r#"
let xs = [i * i for i in 0..6]
let evens = [x for x in xs if x % 2 == 0]
let pairs = [str(a) + b for a in 1..3 for b in ["x", "y"] if a != 2 or b == "y"]
let words = ["a", "bb", "ccc"]
print(xs, evens, pairs, [len(w) for w in words], 2..5, len([w for w in words if len(w) > 1]))
let e: [int] = []
print([x for x in e], [[j for j in 0..i] for i in 0..3])
"#;
    ok(
        src,
        "[0, 1, 4, 9, 16, 25] [0, 4, 16] [\"1x\", \"1y\", \"2y\"] [1, 2, 3] [2, 3, 4] 2\n[] [[], [0], [0, 1]]\n",
    );
    fails(
        limits("check", "let a = [x for x in 5]\nlet b = [print(x) for x in [1]]\nlet c = [x for x in [1] if x]\n", &[]),
        "check error at 1:21: can only iterate over a list or int range, got int\n\
         check error at 2:10: cannot bind a unit value\n\
         check error at 3:28: condition must be bool, got int",
    );
    // Each comprehension iteration is a loop step.
    let o = limits(
        "run",
        "print(len([i for i in 0..1000]))",
        &["--max-steps", "500"],
    );
    fails(o, "runtime error at 1:23: step limit exceeded");
}

#[test]
fn sum_min_max_any_all() {
    let src = r#"
let xs = [3, -1, 7, 2]
let fs = [0.5, 2.25]
print(sum(xs), sum(fs), min(xs), max(xs), min(fs), max(["b", "c", "a"]))
print(min(2, 1), max(2, 9), min("q", "p"), max(1.5, -1.0))
print(any([x > 5 for x in xs]), all([x > 5 for x in xs]), any([false]), all([true]))
let e: [bool] = []
let n: [int] = []
print(any(e), all(e), sum(n))
"#;
    ok(
        src,
        "11 2.75 -1 7 0.5 c\n1 9 p 1.5\ntrue false false true\nfalse true 0\n",
    );
    fails(
        run("let n: [int] = []\nprint(max(n))\n"),
        "runtime error at 2:7: max of empty list",
    );
    fails(
        run("print(sum([9223372036854775807, 1]))\n"),
        "runtime error at 1:7: integer overflow in '+'",
    );
    fails(
        limits("check", "print(sum([\"a\"]), min(1, 2.0), any([1]), max([true]), min(1, 2, 3))\n", &[]),
        "check error at 1:11: 'sum' expects a list of int or float, got [str]\n\
         check error at 1:26: 'min' expects a int like the first argument, got float\n\
         check error at 1:36: 'any' expects a [bool], got [int]\n\
         check error at 1:46: 'max' expects a list of int, float or str (or two values), got [bool]\n\
         check error at 1:55: 'min' takes 1 to 2 argument(s), got 3",
    );
}

#[test]
fn script_mode_rules() {
    fails(
        limits("check", "print(1)\nfn main():\n    print(2)\n", &[]),
        "check error at 2:1: 'fn main' cannot be combined with top-level statements\n  \
         hint: move the top-level statements into main, or delete 'fn main'",
    );
    // Functions are visible before their definition; top-level variables
    // are locals of the program body, not globals.
    ok("print(f())\nfn f() -> int: 3\n", "3\n");
    fails(
        limits("check", "let g = 1\nfn f() -> int: g\nprint(f())\n", &[]),
        "check error at 2:16: undefined variable 'g'",
    );
}

#[test]
fn indentation_errors() {
    fails(
        limits("check", "if true:\n\tprint(1)\n", &[]),
        "lex error at 2:1: tab character: indent with 4 spaces",
    );
    fails(
        limits("check", "if true:\n  print(1)\n", &[]),
        "lex error at 2:3: indentation must be 4 spaces per level (expected 4 spaces, got 2)",
    );
    fails(
        limits(
            "check",
            "if true:\n    if true:\n        print(1)\n  print(2)\n",
            &[],
        ),
        "lex error at 4:3: unindent does not match any outer indentation level",
    );
    fails(
        limits("check", "print(1)\n    print(2)\n", &[]),
        "parse error at 2:5: expected expression, found 'indent'\n  \
         hint: unexpected indentation: indent only after a line ending in ':'",
    );
    fails(
        limits("check", "if true:\nprint(1)\n", &[]),
        "parse error at 2:1: expected an indented block, found 'print'",
    );
    fails(
        limits("check", "let x = 1;\n", &[]),
        "parse error at 1:10: expected end of line, found ';'\n  \
         hint: Limits has no semicolons: put one statement per line",
    );
    fails(
        limits("check", "let x = 1\nif x > 0 {\n    print(x)\n}\n", &[]),
        "parse error at 2:10: expected ':', found '{'\n  \
         hint: blocks are indented: end the line with ':' and indent the body 4 spaces",
    );
    // Blank lines and comments at any indentation are ignored; `pass` is an
    // empty statement.
    ok(
        "if true:\n\n  # odd comment\n    pass\n        # deeper\n    print(1)\nwhile false: pass\n",
        "1\n",
    );
}

#[test]
fn values_and_inout() {
    // Assignment copies (logically); only `inout` parameters change the
    // caller's variable, and the call marks it with `&`.
    let src = r#"
struct P:
    x: int
    tags: [str]
fn bump(p: inout P):
    p.x += 1
    push(p.tags, "b")
fn bumped(p: P) -> P:
    var q = p
    q.x += 100
    q
var p = P { tags: ["a"], x: 1 }
let q = p
bump(&p)
let r = bumped(p)
print(p.x, p.tags, p, q, r.x)
var grid = [[0, 0], [0, 0]]
let row = grid[1]
grid[1][0] = 7
fn set(xs: inout [int], i: int, v: int):
    xs[i] = v
set(&grid[0], 1, 3)
print(grid, row)
"#;
    ok(
        src,
        "2 [\"a\", \"b\"] P {x: 2, tags: [\"a\", \"b\"]} P {x: 1, tags: [\"a\"]} 102\n[[0, 3], [7, 0]] [0, 0]\n",
    );
}

#[test]
fn recursion_and_mutual_recursion() {
    let src = r#"
fn even(n: int) -> bool:
    if n == 0: return true
    odd(n - 1)
fn odd(n: int) -> bool:
    if n == 0: return false
    even(n - 1)
fn fact(n: int) -> int:
    if n <= 1: return 1
    n * fact(n - 1)
print(even(10), odd(7), fact(20))
"#;
    ok(src, "true true 2432902008176640000\n");
}

#[test]
fn shadowing_in_inner_block() {
    ok(
        "let x = 1\nif true:\n    let x = x + 10\n    print(x)\nprint(x)",
        "11\n1\n",
    );
}

#[test]
fn assert_passes_and_comments() {
    ok(
        "# comment\nassert(1 + 1 == 2, \"math\")  # trailing\nprint(\"ok\")",
        "ok\n",
    );
}

#[test]
fn float_formatting() {
    ok(
        "print(0.1 + 0.2, 1000.0, 100.0, -0.5, 10.0 / 4.0, 7.5 % 2.0)",
        "0.30000000000000004 1000.0 100.0 -0.5 2.5 1.5\n",
    );
}

// ---- runtime errors ----

#[test]
fn runtime_index_out_of_bounds() {
    fails(
        run("let xs = [1, 2]\nprint(xs[2])\n"),
        "runtime error at 2:9: index 2 out of bounds for list of length 2",
    );
}

#[test]
fn runtime_negative_index() {
    fails(
        run("var xs = [1]\nxs[-1] = 3\n"),
        "runtime error at 2:3: index -1 out of bounds for list of length 1",
    );
}

#[test]
fn runtime_division_by_zero() {
    fails(
        run("let z = 0\nprint(1 % 1, 5 / z)\n"),
        "runtime error at 2:16: division by zero",
    );
}

#[test]
fn runtime_integer_overflow() {
    fails(
        run("let x = 9223372036854775807\nlet y = x + 1\n"),
        "runtime error at 2:11: integer overflow in '+'",
    );
}

#[test]
fn runtime_overflow_mul_and_neg() {
    fails(
        run("let x = 4611686018427387904\nprint(x * 2)"),
        "runtime error at 2:9: integer overflow in '*'",
    );
}

#[test]
fn runtime_pop_empty() {
    fails(
        run("var xs: [int] = []\npop(xs)\n"),
        "runtime error at 2:1: pop from empty list",
    );
}

#[test]
fn runtime_assert_with_and_without_message() {
    fails(
        run("assert(1 > 2, \"bad \" + str(7))\n"),
        "runtime error at 1:1: assertion failed: bad 7",
    );
    fails(
        run("fn main():\n    assert(false)"),
        "runtime error at 2:5: assertion failed",
    );
}

#[test]
fn runtime_bad_int_conversion() {
    fails(
        run("print(int(\"12x\"))\n"),
        "runtime error at 1:7: invalid int literal",
    );
}

#[test]
fn output_before_error_is_flushed() {
    let o = run("print(\"before\")\nlet xs: [int] = []\nprint(xs[0])");
    assert_eq!(o.stdout, "before\n");
    assert_eq!(o.code, 1);
}

// ---- limits ----

#[test]
fn infinite_loop_hits_step_limit() {
    let o = limits(
        "run",
        "var i = 0\nwhile true:\n    i += 1\n",
        &["--max-steps", "100000"],
    );
    fails(o, "runtime error at 2:7: step limit exceeded");
}

#[test]
fn default_step_limit_stops_infinite_loop() {
    let o = run("while true: pass");
    fails(o, "runtime error at 1:7: step limit exceeded");
}

#[test]
fn infinite_recursion_hits_depth_limit() {
    let o = run("fn f(n: int) -> int:\n    f(n + 1)\nprint(f(0))");
    fails(o, "runtime error at 1:1: call depth limit exceeded");
}

#[test]
fn max_depth_flag() {
    let src = "fn d(n: int) -> int:\n    if n == 0: return 0\n    1 + d(n - 1)\nprint(d(50))";
    assert_eq!(limits("run", src, &["--max-depth", "100"]).stdout, "50\n");
    fails(
        limits("run", src, &["--max-depth", "20"]),
        "runtime error at 1:1: call depth limit exceeded",
    );
}

#[test]
fn step_limit_counts_calls() {
    let src = "fn g(): pass\ng()\ng()\ng()";
    assert_eq!(limits("run", src, &["--max-steps", "4"]).code, 0);
    fails(
        limits("run", src, &["--max-steps", "3"]),
        "runtime error at 1:1: step limit exceeded",
    );
}

#[test]
fn unchecked_removes_limits_but_keeps_bounds() {
    let src =
        "var i = 0\nwhile i < 1000: i += 1\nprint(i)\nlet x = 9223372036854775807\nprint(x + 1)";
    assert_eq!(
        limits("run", src, &["--unchecked", "--max-steps", "10"]).stdout,
        "1000\n-9223372036854775808\n"
    );
    fails(
        limits("run", "let xs = [1]\nprint(xs[5])", &["--unchecked"]),
        "runtime error at 2:9: index 5 out of bounds for list of length 1",
    );
}

// ---- compile-time errors ----

#[test]
fn check_reports_all_errors() {
    let src = r#"
fn f(a: int) -> int:
    return a + 1.0
fn main():
    let x = y
    let s: str = 3
    z = 1
    f("a")
    break
    let e = []
    print(undefined_fn(1))
"#;
    let o = limits("check", src, &[]);
    fails(
        o,
        "check error at 3:14: cannot apply '+' to int and float (use float(x) or int(x) to convert)\n\
         check error at 5:13: undefined variable 'y'\n\
         check error at 6:18: value of 's' must be str, got int\n\
         check error at 7:5: assignment to undeclared variable 'z'\n  hint: declare it first: var z = ...\n\
         check error at 8:7: argument 1 of 'f' must be int, got str\n\
         check error at 9:5: 'break' outside of a loop\n\
         check error at 10:13: cannot infer the type of an empty list; add an annotation like 'var xs: [int] = []'\n\
         check error at 11:11: undefined function 'undefined_fn'",
    );
}

#[test]
fn check_struct_and_return_errors() {
    let src = "struct P:\n    x: int\nfn g() -> int:\n    if true: return 1\nlet p = P { y: 1 }\nprint(p.z)\nlet q: Q = p\n";
    fails(
        limits("check", src, &[]),
        "check error at 3:1: function 'g' may finish without returning an int\n\
         check error at 5:9: field 'x' missing in 'P' literal\n\
         check error at 5:16: struct 'P' has no field 'y'\n\
         check error at 6:8: struct 'P' has no field 'z'\n\
         check error at 7:8: unknown type 'Q'",
    );
}

#[test]
fn check_missing_main() {
    fails(
        limits("check", "pub fn f(): pass", &[]),
        "check error at 1:1: missing 'fn main()'\n  \
         hint: write the program as top-level statements, or add 'fn main():'",
    );
}

#[test]
fn lex_and_parse_errors() {
    fails(
        limits("check", "let s = \"abc\n", &[]),
        "lex error at 1:9: unterminated string",
    );
    fails(
        limits("check", "let x = 1 2\n", &[]),
        "parse error at 1:11: expected end of line, found '2'",
    );
    fails(
        limits("check", "print(1 < 2 < 3)", &[]),
        "parse error at 1:13: comparison operators cannot be chained, found '<'",
    );
}

#[test]
fn build_emits_binary_and_emit_c() {
    let dir = std::env::temp_dir().join(format!("limits-build-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("p.lim");
    std::fs::write(&file, "print(\"built\")\n").unwrap();
    let bin = dir.join("p.bin");
    let st = common::limits()
        .arg("build")
        .arg(&file)
        .arg("-o")
        .arg(&bin)
        .status()
        .unwrap();
    assert!(st.success());
    let o = Command::new(&bin).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout), "built\n");
    let c = common::limits()
        .arg("build")
        .arg(&file)
        .arg("--emit-c")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&c.stdout).contains("int main("));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn examples_run() {
    let root = env!("CARGO_MANIFEST_DIR");
    for (name, needle) in [
        ("hello", "hello, world"),
        ("fib", "fib(30) = 832040"),
        ("structs", "Point {x: 1.5, y: 0.0}"),
        ("primes", "count below 1000000: 78498"),
    ] {
        let o = common::limits()
            .arg("run")
            .arg(format!("{}/examples/{}.lim", root, name))
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{} failed: {}",
            name,
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(
            String::from_utf8_lossy(&o.stdout).contains(needle),
            "{}",
            name
        );
    }
}

#[test]
fn step_accounting_is_exact_across_loops_and_calls() {
    // main: 1 (entry) + 5 loop steps + 5 calls to g, each g charges 1 + 2 loop
    // steps = 1 + 5 + 5 * 3 = 21 steps in total.
    let src = "fn g():\n    for i in 0..2: pass\nfor i in 0..5: g()\nprint(\"done\")";
    assert_eq!(limits("run", src, &["--max-steps", "21"]).stdout, "done\n");
    fails(
        limits("run", src, &["--max-steps", "20"]),
        "runtime error at 2:14: step limit exceeded",
    );
}

#[test]
fn sqrt_builtin() {
    ok("print(sqrt(16.0), sqrt(2.0))", "4.0 1.4142135623730951\n");
    let o = limits("check", "print(sqrt(4))", &[]);
    fails(o, "check error at 1:12: 'sqrt' expects a float, got int");
}

/// Large programs are split into several C files compiled in parallel; the
/// result and the limits must be the same as for a single file.
#[test]
fn large_program_split_into_parallel_c_files() {
    let mut src = String::from("fn f0(n: int) -> int: n\n");
    for i in 1..400 {
        src += &format!(
            "fn f{i}(n: int) -> int:\n    var s = 0\n    for k in 0..3: s += k\n    f{}(n + 1) + s - 3\n",
            i - 1
        );
    }
    src += "print(f399(0))\n";
    let o = limits("build", &src, &["--emit-c", "-o", "/dev/null", "--time"]);
    assert!(o.stderr.contains("C files: 1"), "{}", o.stderr);
    let o = limits("run", &src, &["--time"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(o.stdout, "399\n");
    if std::thread::available_parallelism().map_or(1, |n| n.get()) > 1 {
        assert!(!o.stderr.contains("C files: 1"), "{}", o.stderr);
    }
    // 401 function entries (main + f0..f399) + 399 * 3 loop steps = 1598.
    assert_eq!(limits("run", &src, &["--max-steps", "1598"]).code, 0);
    let o = limits("run", &src, &["--max-steps", "1597"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("step limit exceeded"));
    let o = limits("run", &src, &["--max-depth", "100"]);
    assert!(o.stderr.contains("call depth limit exceeded"));
}

// ---- regressions from review ----

#[test]
fn values_cannot_form_cycles() {
    // Pushing a value into itself stores a copy of the old value: no cycle.
    for flags in [&[][..], &["--unchecked"][..]] {
        let o = limits(
            "run",
            "struct N:\n    v: int\n    next: [N]\nvar n = N { v: 1, next: [] }\npush(n.next, n)\npush(n.next, n)\nprint(n)\nvar xs = [[0]]\npush(xs, xs[0])\nxs[1][0] = 5\npush(xs[0], len(xs))\nprint(xs)",
            flags,
        );
        assert_eq!(o.stderr, "");
        assert_eq!(
            o.stdout,
            "N {v: 1, next: [N {v: 1, next: []}, N {v: 1, next: [N {v: 1, next: []}]}]}\n[[0, 2], [5]]\n"
        );
    }
}

#[test]
fn str_of_deep_chain_is_a_runtime_error() {
    let o = run(
        "struct N:\n    v: int\n    next: [N]\nvar h = N { v: 0, next: [] }\nfor i in 0..300000: h = N { v: i, next: [h] }\nprint(len(str(h)))",
    );
    fails(
        o,
        "runtime error at 6:11: value nested too deeply to format (cyclic?)",
    );
}

#[test]
fn int_min_div_minus_one_names_division() {
    fails(
        run("let m = -9223372036854775807 - 1\nprint(m / -1)"),
        "runtime error at 2:9: integer overflow in '/'",
    );
}

#[test]
fn int_min_literal() {
    ok(
        "let x = -9223372036854775808\nprint(x, x + 1)",
        "-9223372036854775808 -9223372036854775807\n",
    );
    fails(
        limits("check", "let x = 9223372036854775808", &[]),
        "lex error at 1:9: integer literal '9223372036854775808' out of range",
    );
}

#[test]
fn self_containing_struct_rejected() {
    fails(
        limits("check", "struct S:\n    s: S\nprint(1)", &[]),
        "check error at 1:1: struct 'S' contains itself (only through a list can a struct refer to itself), so it can never be constructed",
    );
}

#[test]
fn struct_named_like_builtin_type_rejected() {
    fails(
        limits("check", "struct int:\n    x: int\nfn main(): pass", &[]),
        "check error at 1:1: 'int' is a builtin type name",
    );
}

#[test]
fn float_specials_lowercase() {
    ok(
        "print(1.0/0.0, -1.0/0.0, 0.0/0.0, str(0.0/0.0))",
        "inf -inf nan nan\n",
    );
}

#[test]
fn strict_number_parsing() {
    for (src, msg) in [
        ("int(\"\\t5\")", "invalid int literal"),
        ("int(\" 5\")", "invalid int literal"),
        ("float(\"0x10\")", "invalid float literal"),
        ("float(\"nan\")", "invalid float literal"),
        ("float(\"inf\")", "invalid float literal"),
    ] {
        let o = run(&format!("print({})", src));
        fails(o, &format!("runtime error at 1:7: {}", msg));
    }
    ok(
        r#"print(int("-12"), int("+7"), float("1.5e3"), float("-.5"))"#,
        "-12 7 1500.0 -0.5\n",
    );
}

#[test]
fn big_frames_hit_depth_limit_not_segfault() {
    let n = 300;
    let lets: String = (0..n)
        .map(|i| format!("    let a{} = n * {}\n", i, i + 1))
        .collect();
    let sum: Vec<String> = (0..n).map(|i| format!("a{}", i)).collect();
    let src = format!(
        "fn f(n: int) -> int:\n{}    if n == 0: return 0\n    let r = f(n - 1)\n    r + {}\nprint(f(9990))\n",
        lets,
        sum.join(" + ")
    );
    fails(run(&src), "runtime error at 1:1: call depth limit exceeded");
}

#[test]
fn builtins_charge_steps() {
    let src = "var s = \"a\"\nfor i in 0..28: s = s + s\nvar n = 0\nfor k in 0..300: n += len(s)\nprint(n)";
    fails(
        limits("run", src, &["--max-steps", "1000"]),
        "runtime error at 2:23: step limit exceeded",
    );
}

#[test]
fn memory_budget() {
    let src = "var s = \"xxxxxxxxxxxxxxxx\"\nwhile true:\n    s = s + s\n";
    for flags in [&[][..], &["--unchecked"][..]] {
        fails(
            limits("run", src, flags),
            "runtime error at 3:11: memory limit exceeded",
        );
    }
    let o = limits(
        "run",
        "var xs = [0]\nwhile true: push(xs, 1)",
        &["--max-mem", "8"],
    );
    fails(o, "runtime error at 2:13: memory limit exceeded");
}

#[test]
fn compiler_rejects_too_deep_nesting() {
    let chain = vec!["1"; 20000].join(" + ");
    let o = limits("check", &format!("let x = {}", chain), &[]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("parse error") && o.stderr.contains("nesting too deep"));
    let parens = format!("let x = {}1{}", "(".repeat(100000), ")".repeat(100000));
    let o = limits("check", &parens, &[]);
    assert!(o.stderr.contains("nesting too deep"), "{}", o.stderr);
    let ifs: String = (0..5000)
        .map(|i| format!("{}if true:\n", "    ".repeat(i)))
        .collect::<String>()
        + &"    ".repeat(5000)
        + "pass\n";
    let o = limits("check", &ifs, &[]);
    assert!(o.stderr.contains("nesting too deep"), "{}", o.stderr);
    let inline = "if true: ".repeat(50000) + "pass\n";
    let o = limits("check", &inline, &[]);
    assert!(o.stderr.contains("nesting too deep"), "{}", o.stderr);
    ok(&format!("print({})", vec!["1"; 2500].join(" + ")), "2500\n");
}

#[test]
fn run_reports_signal_death() {
    // --unchecked drops the depth/stack check, so runaway recursion dies by SIGSEGV.
    let o = limits(
        "run",
        "fn f(xs: inout [int]):\n    push(xs, 1)\n    f(&xs)\n    print(len(xs))\nvar a = [0]\nf(&a)",
        &["--unchecked"],
    );
    assert_eq!(o.code, 139);
    assert_eq!(o.stderr.trim_end(), "limits: program killed by signal 11");
}

#[test]
fn packed_lists_of_every_element_type() {
    ok(
        "struct P:\n    x: int\n\
         var bs = [true, false]\npush(bs, true)\nbs[1] = true\nprint(bs, pop(bs), len(bs))\n\
         var fs = [1.5]\npush(fs, 2.0)\nfs[0] = fs[1] * 2.0\nprint(fs)\n\
         var ss: [str] = []\npush(ss, \"a\")\npush(ss, str(bs))\nprint(ss, ss[1])\n\
         var ps = [P {x: 1}]\npush(ps, P {x: 2})\nps[0].x = 7\n\
         var t = 0\nfor p in ps: t += p.x\nprint(ps, t, pop(ps).x)\n\
         var nest = [[1, 2], [3]]\npush(nest[1], 4)\nprint(nest, nest[1][1])\n\
         var big: [bool] = []\nfor i in 0..1000: push(big, i % 3 == 0)\n\
         var c = 0\nfor b in big:\n    if b: c += 1\nprint(c, big[999])\n",
        "[true, true, true] true 2\n[4.0, 2.0]\n[\"a\", \"[true, true]\"] [true, true]\n[P {x: 7}, P {x: 2}] 9 2\n[[1, 2], [3, 4]] 4\n334 true\n",
    );
}

#[test]
fn json_diagnostics() {
    let o = limits("check", "let total = 1\nprint(totl)\n", &["--json"]);
    assert_eq!(o.code, 1);
    let l = o.stderr.trim_end();
    assert!(
        l.starts_with(r#"{"kind":"check","code":"E0201","file":""#),
        "{}",
        l
    );
    assert!(l.ends_with(r#"t.lim","line":2,"col":7,"end_line":2,"end_col":11,"msg":"undefined variable 'totl'","hint":"did you mean 'total'?","source_line":"print(totl)","expected":null,"found":null,"alternatives":["total","env"],"fixes":[{"msg":"rename 'totl' to 'total'","edits":[{"line":2,"col":7,"end_line":2,"end_col":11,"text":"total"}],"applicability":"machine"}]}"#), "{}", l);
    let o = limits("run", "assert(false, \"a \\\"q\\\"\")\n", &["--json"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains(r#""code":"E0506","#), "{}", o.stderr);
    assert!(
        o.stderr.trim_end().ends_with(
            r#""line":1,"col":1,"msg":"assertion failed: a \"q\"","hint":"","data":{}}"#
        ),
        "{}",
        o.stderr
    );
    let o = limits("check", "let x = 1 +", &["--json"]);
    assert!(
        o.stderr.starts_with(r#"{"kind":"parse","code":"E0100","#),
        "{}",
        o.stderr
    );
    // New codes: tabs, indentation, assignment to an immutable binding,
    // top-level statements in a module / next to main, empty min/max.
    for (src, cmd, code) in [
        ("\tx", "check", "E0006"),
        ("if true:\n   pass", "check", "E0007"),
        ("let x = 1\nx = 2", "check", "E0222"),
        ("print(1)\nfn main(): pass", "check", "E0224"),
        ("let e: [int] = []\nprint(min(e))", "run", "E0516"),
    ] {
        let o = limits(cmd, src, &["--json"]);
        assert!(
            o.stderr.contains(&format!("\"code\":\"{}\"", code)),
            "{}: {}",
            src,
            o.stderr
        );
    }
}

#[test]
fn inline_tests_run_isolated() {
    let src = r#"
fn f(n: int) -> int: n + 1
test "passes": assert(f(1) == 2)
test "loops forever":
    while true: pass
test "bad index":
    let xs = [1]
    print(xs[5])
test "still runs": assert(f(0) == 1, "f")
print("main")
"#;
    let o = limits("test", src, &["--max-steps", "1000"]);
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert_eq!(
        o.stdout,
        "ok passes\nFAIL loops forever: runtime error at 5:11: step limit exceeded\n\
         FAIL bad index: runtime error at 8:13: index 5 out of bounds for list of length 1\n\
         ok still runs\n2 passed, 2 failed\n"
    );
    // Tests are type checked but not part of build/run output.
    ok(src, "main\n");
    assert_eq!(limits("test", "test \"t\": pass", &[]).code, 0);
}

#[test]
fn contracts() {
    let src = r#"
fn isqrt(n: int) -> int:
    requires n >= 0
    ensures result * result <= n
    ensures (result + 1) * (result + 1) > n
    var r = 0
    while (r + 1) * (r + 1) <= n: r += 1
    if n == 99: return r + 1
    r
print(isqrt(17))
print(isqrt(ARG))
"#;
    let o = run(&src.replace("ARG", "-1"));
    assert_eq!(o.stdout, "4\n");
    fails(o, "runtime error at 3:16: requires failed in fn isqrt");
    fails(
        run(&src.replace("ARG", "99")),
        "runtime error at 4:29: ensures failed in fn isqrt",
    );
    let o = limits("run", &src.replace("ARG", "-1"), &["--unchecked"]);
    assert_eq!((o.code, o.stdout.as_str()), (0, "4\n0\n"));
    fails(
        limits("check", "fn f():\n    ensures result\n    pass\nf()\n", &[]),
        "check error at 2:13: undefined variable 'result'\n  \
         hint: 'result' only exists in 'ensures' of a fn with a return type",
    );
    // `requires` is an ordinary name outside the head of a fn body.
    ok("var requires = 1\nrequires += 1\nprint(requires)", "2\n");
}

/// Like `limits`, but runs in a fresh directory that is also the program's
/// working directory (the default fs sandbox), with `files` created first.
fn in_dir(
    src: &str,
    flags: &[&str],
    files: &[(&str, &str)],
    stdin: &str,
) -> (Out, std::path::PathBuf) {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!(
        "limits-io-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("t.lim"), src).unwrap();
    for (p, d) in files {
        std::fs::write(dir.join(p), d).unwrap();
    }
    let mut ch = common::limits()
        .arg("run")
        .arg("t.lim")
        .arg("--leak-check")
        .args(flags)
        .current_dir(&dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // The program may stop reading early (budget errors): ignore EPIPE.
    let _ = ch.stdin.take().unwrap().write_all(stdin.as_bytes());
    let o = ch.wait_with_output().unwrap();
    let out = Out {
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        code: o.status.code().unwrap_or(-1),
    };
    (out, dir)
}

#[test]
fn io_builtins_with_capabilities() {
    let src = r#"uses fs.read, fs.write, env, time, args, stdin
env.fs.write("sub/o.txt", env.fs.read("in.txt") + "!")
print(env.fs.read("sub/o.txt"), env.vars.get("LIMITS_NO_SUCH_VAR") == "", env.clock.now_ms() > 0, env.args.list())
let i = env.stdin
print(i.read_line(), i.read_line(), i.read_line() == "")
"#;
    let (o, dir) = in_dir(src, &["--", "a", "b c"], &[("in.txt", "hi")], "one\ntwo");
    let _ = std::fs::remove_dir_all(dir);
    assert_eq!((o.code, o.stderr.as_str()), (0, ""));
    assert_eq!(o.stdout, "hi! true true [\"a\", \"b c\"]\none two true\n");
}

#[test]
fn io_builtins_need_declared_capabilities() {
    fails(
        limits("check", "uses net\nprint(env.fs.read(\"x\"))\n", &[]),
        "check error at 1:6: unknown capability 'net'\n  hint: capabilities are: fs.read, fs.write, env, time, args, stdin\ncheck error at 2:7: 'Fs.read' needs capability 'fs.read'\n  hint: add 'uses fs.read' at the top of this file",
    );
}

#[test]
fn capabilities_are_values_passed_to_functions() {
    // An fn does I/O only through a capability parameter; fs.sub narrows.
    let src = r#"uses fs.read, fs.write, args
fn load(fs: Fs, name: str) -> str: fs.read(name)
fn save(fs: Fs, name: str, data: str): fs.write(name, data)
fn main(env: Env):
    let d = env.fs.sub("sub")
    save(d, "o.txt", load(env.fs, "in.txt") + "?")
    print(load(d, "o.txt"), load(env.fs, "sub/o.txt"), len(env.args.list()))
    let dd = d.sub(".")
    print(dd.read("o.txt"))
    print(d.read(env.args.list()[0]))
"#;
    let (o, dir) = in_dir(src, &["--", "o.txt"], &[("in.txt", "hi")], "");
    let _ = std::fs::remove_dir_all(dir);
    assert_eq!((o.code, o.stderr.as_str()), (0, ""));
    assert_eq!(o.stdout, "hi? hi? 1\nhi?\nhi?\n");
    // A narrowed Fs cannot reach outside its directory, by `..`, an
    // absolute path or a sub() of its own.
    for arg in ["../in.txt", "/etc/hostname"] {
        let (o, dir) = in_dir(src, &["--", arg], &[("in.txt", "hi")], "");
        let root = std::fs::canonicalize(dir.join("sub")).unwrap();
        let _ = std::fs::remove_dir_all(dir);
        fails(
            o,
            &format!(
                "runtime error at 10:11: access denied: '{}' is outside the Fs root '{}'",
                arg,
                root.display()
            ),
        );
    }
    let src = "uses fs.read\nprint(env.fs.sub(\"sub\").sub(\"..\").read(\"t.lim\"))\n";
    let (o, dir) = in_dir(src, &[], &[], "");
    let root = std::fs::canonicalize(dir.join("sub")).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    fails(
        o,
        &format!(
            "runtime error at 2:7: access denied: '..' is outside the Fs root '{}'",
            root.display()
        ),
    );
    let src = "uses fs.read\nprint(env.fs.sub(\"nope\").read(\"x\"))\n";
    let (o, dir) = in_dir(src, &[], &[], "");
    let _ = std::fs::remove_dir_all(dir);
    fails(
        o,
        "runtime error at 2:7: cannot use directory 'nope': No such file or directory",
    );
}

#[test]
fn capabilities_cannot_be_stored_or_used_without_env() {
    let src = r#"uses fs.read
struct S:
    f: Fs
fn g(xs: [Clock]): pass
fn h(): print(env.fs.read("x"))
let a = [env.fs]
let b = Some(env.vars)
print(env.fs)
print(read_file("x"))
env.fs.reed("x")
env.clock.now_ms()
fn main2(n: int): n.foo()
"#;
    fails(
        limits("check", src, &[]),
        "check error at 2:1: a capability (Fs) cannot be stored in a struct field
  hint: pass capabilities as fn parameters instead (e.g. 'fn load(fs: Fs)')
check error at 4:11: a capability (Clock) cannot be stored in a list
  hint: pass capabilities as fn parameters instead (e.g. 'fn load(fs: Fs)')
check error at 5:15: undefined variable 'env'
  hint: 'env' exists only at the top level of the root file (or as 'fn main(env: Env)'); pass what a fn needs as a parameter, e.g. 'fn load(fs: Fs)' called as load(env.fs)
check error at 6:9: a capability (Fs) cannot be stored in a list
  hint: pass capabilities as fn parameters instead (e.g. 'fn load(fs: Fs)')
check error at 7:9: a capability (Vars) cannot be stored in an Option
  hint: pass capabilities as fn parameters instead (e.g. 'fn load(fs: Fs)')
check error at 8:7: 'print' expects a printable value, got Fs
check error at 9:7: undefined function 'read_file'
  hint: I/O goes through capability values: use env.fs.read(path) (in a fn: take 'fs: Fs' and call fs.read(path))
check error at 10:1: Fs has no method 'reed'
  hint: did you mean 'read'?
check error at 11:1: 'Clock.now_ms' needs capability 'time'
  hint: add 'uses time' at the top of this file
check error at 12:19: int has no method 'foo'
  hint: only capabilities (Fs, Vars, Clock, Args, Stdin) have methods; call functions as f(x)",
    );
    fails(
        limits("check", "fn main(n: int): pass\n", &[]),
        "check error at 1:1: 'main' must take no parameters or '(env: Env)' and return nothing",
    );
}

#[test]
fn show_perms_prints_the_manifest() {
    let o = limits(
        "run",
        "uses stdin, fs.read, env\nprint(1)\n",
        &["--show-perms", "--allow", "env=HOME"],
    );
    assert_eq!((o.code, o.stderr.as_str()), (0, ""));
    let o2 = o.stdout.lines().skip(1).collect::<Vec<_>>().join("\n");
    assert_eq!(
        o2,
        "uses: fs.read, env, stdin\n  fs.read: current directory at run time\n  env: HOME"
    );
    let o = limits("check", "print(1)\n", &["--show-perms"]);
    assert!(
        o.stdout.ends_with("uses: (none: compute and print only)\n"),
        "{}",
        o.stdout
    );
}

#[test]
fn fs_access_is_sandboxed() {
    let src = "uses fs.read, fs.write, args\nlet p = env.args.list()[0]\nif p == \"w\": env.fs.write(\"../x\", \"\")\nprint(len(env.fs.read(p)))\n";
    let cases: &[(&[&str], &str)] = &[
        (&["--", "../t.lim"], "runtime error at 4:11: access denied: '../t.lim' is outside the allowed fs.read paths (see --allow)"),
        (&["--", "sub/../../t.lim"], "runtime error at 4:11: access denied: 'sub/../../t.lim' is outside the allowed fs.read paths (see --allow)"),
        (&["--", "w"], "runtime error at 3:14: access denied: '../x' is outside the allowed fs.write paths (see --allow)"),
        (&["--", "nope"], "runtime error at 4:11: cannot read file 'nope': No such file or directory"),
        (&["--", "sub"], "runtime error at 4:11: cannot read file 'sub': not a regular file"),
        (&["--allow", "fs.read=sub", "--", "t.lim"], "runtime error at 4:11: access denied: 't.lim' is outside the allowed fs.read paths (see --allow)"),
    ];
    for (flags, want) in cases {
        let (o, dir) = in_dir(src, flags, &[], "");
        let _ = std::fs::remove_dir_all(dir);
        fails(o, want);
    }
    // A symlink inside the sandbox pointing out of it is an escape.
    let (_, dir) = in_dir(src, &["--", "x"], &[], "");
    std::os::unix::fs::symlink("/etc/hostname", dir.join("link")).unwrap();
    let o = common::limits()
        .args(["run", "t.lim", "--", "link"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        String::from_utf8_lossy(&o.stderr).trim_end(),
        "runtime error at 4:11: access denied: 'link' is outside the allowed fs.read paths (see --allow)"
    );
}

#[test]
fn file_reads_count_steps_and_memory() {
    let src = "uses fs.read\nprint(len(env.fs.read(\"big\")))\n";
    let big = "x".repeat(1 << 20);
    let (o, dir) = in_dir(src, &["--max-steps", "1000"], &[("big", &big)], "");
    let _ = std::fs::remove_dir_all(dir);
    fails(o, "runtime error at 2:11: step limit exceeded");
    let (o, dir) = in_dir(src, &["--max-mem", "1"], &[("big", &big)], "");
    let _ = std::fs::remove_dir_all(dir);
    fails(o, "runtime error at 2:11: memory limit exceeded");
}

#[test]
fn split_join_sort() {
    ok(
        r#"var p = split("b,a,,c", ",")
print(len(p), p, split("", ","), split("hé!", ""), split("x", ""))
print(join(p, "+"), join(split("a b", " "), ""))
let e: [str] = []
print(join(e, ","), len(join(e, ",")))
sort(p)
print(p)
var xs = [3, -1, 2, 2]
sort(xs)
var fs = [2.5, -0.5, 1.0]
sort(fs)
print(xs, fs)"#,
        "4 [\"b\", \"a\", \"\", \"c\"] [\"\"] [\"h\", \"é\", \"!\"] [\"x\"]\nb+a++c ab\n 0\n[\"\", \"a\", \"b\", \"c\"]\n[-1, 2, 2, 3] [-0.5, 1.0, 2.5]\n",
    );
    let o = run("sort([true])\nlet n = join([1], \",\")\nsplit(1, 2)");
    assert!(
        o.stderr
            .contains("'sort' expects a list of int, float or str, got [bool]"),
        "{}",
        o.stderr
    );
    assert!(
        o.stderr.contains("'join' expects a [str], got [int]"),
        "{}",
        o.stderr
    );
    assert!(
        o.stderr.contains("'split' expects a str, got int"),
        "{}",
        o.stderr
    );
    assert!(
        o.stderr
            .contains("'split' expects a str separator, got int"),
        "{}",
        o.stderr
    );
    // sort and split are charged against the step budget
    let big =
        "var xs: [int] = []\nfor i in 0..100000: push(xs, 100000 - i)\nsort(xs)\nprint(xs[0])";
    let o = limits("run", big, &["--max-steps", "1000000"]);
    assert!(o.stderr.contains("step limit exceeded"), "{}", o.stderr);
}

/// Run `limits <args>` in `dir` and return (stdout, stderr, exit code).
fn cmd_in(dir: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let o = common::limits()
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (s(&o.stdout), s(&o.stderr), o.status.code().unwrap_or(-1))
}

#[test]
fn fifo_is_rejected_without_blocking() {
    let src = "uses fs.read, fs.write, args\nif len(env.args.list()) > 0: env.fs.write(\"fifo\", \"x\")\nprint(env.fs.read(\"fifo\"))\n";
    let (_, dir) = in_dir("pass", &[], &[], "");
    std::fs::write(dir.join("t.lim"), src).unwrap();
    assert!(Command::new("mkfifo")
        .arg(dir.join("fifo"))
        .status()
        .unwrap()
        .success());
    let (_, r, _) = cmd_in(&dir, &["run", "t.lim"]);
    let (_, w, _) = cmd_in(&dir, &["run", "t.lim", "--", "w"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        r.trim_end(),
        "runtime error at 3:7: cannot read file 'fifo': not a regular file"
    );
    assert_eq!(
        w.trim_end(),
        "runtime error at 2:30: cannot write file 'fifo': not a regular file"
    );
}

#[test]
fn input_line_keeps_nul_bytes_and_charges_steps() {
    let src =
        "uses stdin\nlet s = env.stdin.read_line()\nprint(len(s), s == \"x\" + split(\"a\", \"a\")[0])\n";
    let (o, dir) = in_dir(src, &[], &[], "x\0yz\n");
    let _ = std::fs::remove_dir_all(dir);
    assert_eq!(o.stdout, "4 false\n");
    let zeros = "\0".repeat(1 << 20);
    let (o, dir) = in_dir(src, &["--max-steps", "100"], &[], &zeros);
    let _ = std::fs::remove_dir_all(dir);
    fails(o, "runtime error at 2:9: step limit exceeded");
}

#[test]
fn test_names_are_escaped() {
    let src = "test \"x\\nok 5 passed, 0 failed\\nPASS \\\"q\": assert(false)\n";
    let o = limits("test", src, &["--json"]);
    assert_eq!(o.stdout.lines().count(), 2, "{}", o.stdout);
    assert!(
        o.stdout
            .starts_with("FAIL x\\nok 5 passed, 0 failed\\nPASS \\\"q: {\"kind\":\"runtime\""),
        "{}",
        o.stdout
    );
}

#[test]
fn json_runtime_errors_are_valid_utf8() {
    let src = "uses fs.read\nlet p = env.fs.read(\"bin\")\nprint(env.fs.read(\"../\" + p))\n";
    let (_, dir) = in_dir(src, &[], &[], "");
    std::fs::write(dir.join("bin"), b"a\xffb\xe2\x82\xac").unwrap();
    let o = common::limits()
        .args(["run", "t.lim", "--json"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let err = String::from_utf8(o.stderr).expect("valid UTF-8");
    assert!(err.contains("'../a\\ufffdb€'"), "{}", err);
}

#[test]
fn non_utf8_source_is_a_located_lex_error() {
    let dir = std::env::temp_dir().join(format!("limits-u8-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("u.lim"), b"fn main():\n    print(\"\xff\")\n").unwrap();
    let (_, e, code) = cmd_in(&dir, &["check", "u.lim", "--json"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(code, 1);
    assert_eq!(
        e.trim_end(),
        "{\"kind\":\"lex\",\"code\":\"E0005\",\"file\":\"u.lim\",\"line\":2,\"col\":12,\"end_line\":2,\"end_col\":13,\"msg\":\"invalid UTF-8 in source file\",\"hint\":\"\",\"source_line\":\"    print(\\\"\u{fffd}\\\")\",\"expected\":null,\"found\":null,\"alternatives\":[],\"fixes\":[]}"
    );
}

#[test]
fn small_memory_budget_charges_bytes_not_chunks() {
    let src = "struct P:\n    x: int\nlet p = P { x: 1 }\nlet xs = [[1]]\nprint(p, xs)\n";
    let o = limits("run", src, &["--max-mem", "1"]);
    assert_eq!(
        (o.stdout.as_str(), o.code),
        ("P {x: 1} [[1]]\n", 0),
        "{}",
        o.stderr
    );
    let grow = "var xs = [\"a\"]\nwhile true: push(xs, xs[0] + \"b\")";
    let o = limits("run", grow, &["--max-mem", "1"]);
    fails(o, "runtime error at 2:28: memory limit exceeded");
}

#[test]
fn floats_print_without_exponents() {
    ok(
        "let f = 100000000000000000000.0\nprint(f, [f, 0.000001], str(f) == \"100000000000000000000.0\", float(\"1.25e-7\"), -0.00012)",
        "100000000000000000000.0 [100000000000000000000.0, 0.000001] true 0.000000125 -0.00012\n",
    );
}

#[test]
fn print_writes_nothing_when_an_argument_fails() {
    let o = run("fn f() -> int:\n    print(\"inner\")\n    3\nprint(1, f())\nprint(len(\"\"), 1, int(\"\"), 2)\n");
    assert_eq!(o.stdout, "inner\n1 3\n");
    assert_eq!(
        o.stderr.trim_end(),
        "runtime error at 5:19: invalid int literal"
    );
}

#[test]
fn test_runner_survives_closed_stdout_and_timeouts() {
    let dir = std::env::temp_dir().join(format!("limits-tt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let many: String = (0..20)
        .map(|i| format!("test \"t{}\": pass\n", i))
        .collect();
    std::fs::write(dir.join("t1.lim"), many).unwrap();
    // A test that never ends (no step budget under --unchecked).
    let t2 = "fn r() -> int:\n    while true: print(1)\n    0\ntest \"rec\": print(r())\ntest \"after\": pass\n";
    std::fs::write(dir.join("t2.lim"), t2).unwrap();
    let mut ch = common::limits()
        .args(["test", "t1.lim"])
        .current_dir(&dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(ch.stdout.take()); // closed pipe, like `| head -0`
    let o = ch.wait_with_output().unwrap();
    assert!(!String::from_utf8_lossy(&o.stderr).contains("panicked"));
    let (out, _, code) = cmd_in(&dir, &["test", "t2.lim", "--unchecked"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(code, 1);
    assert_eq!(
        out,
        "FAIL rec: timed out after 10 s\nok after\n1 passed, 1 failed\n"
    );
}

/// Reviewer regressions: stdin at EOF / an empty line must not pass a NULL
/// buffer to memcpy (found by UBSan), checked here under the sanitizer.
#[test]
fn read_line_on_empty_input_is_free_of_undefined_behaviour() {
    if Command::new("gcc").arg("--version").output().is_err() {
        return;
    }
    let dir = std::env::temp_dir().join(format!(
        "limits-ubsan-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("t.lim"),
        "uses stdin\nprint(env.stdin.read_line())\nprint(env.stdin.read_line())\n",
    )
    .unwrap();
    let st = common::limits()
        .args(["build", "t.lim", "--emit-c", "--leak-check", "-o", "t.c"])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(st.success());
    let st = Command::new("gcc")
        .args([
            "-std=c11",
            "-g",
            "-fsanitize=undefined",
            "-fno-sanitize-recover=all",
        ])
        .args(["t.c", "-o", "t", "-lm"])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(st.success());
    // One empty line, then EOF.
    let o = Command::new(dir.join("t"))
        .stdin({
            std::fs::write(dir.join("in"), "\n").unwrap();
            std::fs::File::open(dir.join("in")).unwrap()
        })
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(String::from_utf8_lossy(&o.stderr), "");
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "\n\n");
}

/// An empty path is a runtime I/O error under every C compiler (gcc -O2 used
/// to reject the generated C with -Werror=array-bounds).
#[test]
fn empty_fs_path_is_a_runtime_error_with_cc() {
    for cc in ["cc", "tcc"] {
        let (o, dir) = in_dir(
            "uses fs.read\nprint(env.fs.read(\"\"))\n",
            &["--cc", cc],
            &[],
            "",
        );
        let _ = std::fs::remove_dir_all(&dir);
        fails(
            o,
            "runtime error at 2:7: cannot read file '': Invalid argument",
        );
        let (o, dir) = in_dir(
            "uses fs.write\nenv.fs.write(\"\", \"x\")\n",
            &["--cc", cc],
            &[],
            "",
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(o.code, 1, "{}", o.stderr);
        assert!(
            o.stderr
                .starts_with("runtime error at 2:1: cannot write file ''"),
            "{}",
            o.stderr
        );
    }
}

#[test]
fn match_value_whose_arms_all_leave_is_a_check_error() {
    fails(
        run("fn f(x: int) -> int:\n    let v = match x:\n        _: return 5\n    v\nprint(f(1))\n"),
        "check error at 2:13: every arm of this match leaves (return, break or continue), so it has no value\n  hint: write the match as a statement instead of binding or using its value",
    );
    fails(
        run("for i in 0..3:\n    let v: int = match i:\n        _: continue\n    print(v)\n"),
        "check error at 2:18: every arm of this match leaves (return, break or continue), so it has no value\n  hint: write the match as a statement instead of binding or using its value",
    );
    // One arm with a value is enough.
    ok(
        "fn f(x: Option[int]) -> int:\n    let v = match x:\n        Some(a): a\n        None: return 0\n    v + 1\nprint(f(Some(4)), f(None))\n",
        "5 0\n",
    );
}

#[test]
fn code_after_return_is_reported_as_unreachable() {
    fails(
        run("fn f() -> int:\n    return 1\n    print(\"x\")\nprint(f())\n"),
        "check error at 3:5: unreachable code: the statement before it always leaves (return, break or continue)\n  hint: remove the unreachable statements",
    );
    fails(
        run("for i in 0..2:\n    break\n    print(i)\n"),
        "check error at 3:5: unreachable code: the statement before it always leaves (return, break or continue)\n  hint: remove the unreachable statements",
    );
}

#[test]
fn wildcard_after_exhaustive_arms_is_unreachable() {
    fails(
        run("let b = true\nlet x = match b:\n    true: 1\n    false: 0\n    _: 2\nprint(x)\n"),
        "check error at 5:5: unreachable match arm: the earlier arms already cover every bool\n  hint: remove the '_' arm",
    );
    fails(
        run("enum E:\n    A\n    B\nlet e = E.A\nmatch e:\n    A: print(1)\n    B: print(2)\n    _: print(3)\n"),
        "check error at 8:5: unreachable match arm: the earlier arms already cover every E\n  hint: remove the '_' arm",
    );
}

#[test]
fn diagnostics_use_the_right_article() {
    fails(
        run("fn g(e: Env) -> Result[Fs, str]:\n    Ok(e.fs)\nprint(1)\n"),
        "check error at 1:17: a capability (Fs) cannot be stored in a Result\n  hint: pass capabilities as fn parameters instead (e.g. 'fn load(fs: Fs)')",
    );
    fails(
        run("fn f(x: int) -> int:\n    if x > 0:\n        return 1\nprint(f(1))\n"),
        "check error at 1:1: function 'f' may finish without returning an int",
    );
}

#[test]
fn parse_float_rejects_infinity_consistently() {
    ok(
        "print(parse_float(\"1e400\"), parse_float(\"-1e400\"), parse_float(\"inf\"), parse_float(\"nan\"), parse_float(\"1e5\"), parse_float(\"-2\"))\n",
        "None None None None Some(100000.0) Some(-2.0)\n",
    );
}

#[test]
fn unicode_escapes_are_not_accepted() {
    fails(
        run("print(\"\\u0041\")\n"),
        "lex error at 1:8: unknown escape",
    );
}
