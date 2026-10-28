//! End-to-end tests: compile real .lim programs with limits and run them.

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
    let o = Command::new(env!("CARGO_BIN_EXE_limits"))
        .arg(cmd)
        .arg(&file)
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

fn main_body(body: &str) -> String {
    format!("fn main() {{\n{}\n}}\n", body)
}

#[test]
fn hello_print_mixed() {
    ok(
        r#"fn main() { print("hi", 1, -2.5, true, 3.0); }"#,
        "hi 1 -2.5 true 3.0\n",
    );
}

#[test]
fn arithmetic_and_precedence() {
    ok(
        &main_body("print(1 + 2 * 3, (1 + 2) * 3, 7 / 2, 7 % 3, -7 / 2, 2.0 * 0.25, 1.0 / 3.0);"),
        "7 9 3 1 -3 0.5 0.3333333333333333\n",
    );
}

#[test]
fn comparisons_and_logic() {
    ok(
        &main_body(
            r#"print(1 < 2, 2 <= 1, "a" < "b", "x" == "x", 1 != 1, true and not false, false or false);"#,
        ),
        "true false true true false true false\n",
    );
}

#[test]
fn string_concat_len_and_str() {
    ok(
        &main_body(r#"let s = "ab" + "cé"; print(s, len(s), str(42) + str(1.5) + str(true));"#),
        "abcé 4 421.5true\n",
    );
}

#[test]
fn conversions() {
    ok(
        &main_body(r#"print(int(3.9), int(-3.9), float(2), int("123"), float("2.5") + 1.0);"#),
        "3 -3 2.0 123 3.5\n",
    );
}

#[test]
fn if_else_chain() {
    let src = r#"
fn sign(x: int) -> str {
    if x < 0 { return "neg"; } else if x == 0 { return "zero"; } else { return "pos"; }
}
fn main() { print(sign(-5), sign(0), sign(9)); }
"#;
    ok(src, "neg zero pos\n");
}

#[test]
fn while_break_continue() {
    ok(&main_body("let i = 0; let s = 0; while true { i = i + 1; if i > 10 { break; } if i % 2 == 0 { continue; } s = s + i; } print(s);"), "25\n");
}

#[test]
fn for_range_and_list() {
    ok(&main_body("let t = 0; for i in 0..5 { t = t + i; } let xs = [10, 20, 30]; for v in xs { t = t + v; } for i in 3..1 { t = 0; } print(t);"), "70\n");
}

#[test]
fn for_range_var_mutation_does_not_affect_iteration() {
    ok(
        &main_body("for i in 0..3 { print(i); i = 100; }"),
        "0\n1\n2\n",
    );
}

#[test]
fn lists_push_pop_index_assign() {
    ok(&main_body("let xs: [int] = []; push(xs, 1); push(xs, 2); push(xs, 3); xs[0] = 9; let p = pop(xs); print(xs, p, len(xs));"), "[9, 2] 3 2\n");
}

#[test]
fn nested_lists_and_strings_print() {
    ok(
        &main_body(
            r#"let m = [[1, 2], [3]]; let e: [[str]] = [[], ["a\"b"]]; print(m, e, m[0][1]);"#,
        ),
        "[[1, 2], [3]] [[], [\"a\\\"b\"]] 2\n",
    );
}

#[test]
fn structs_are_references() {
    let src = r#"
struct P { x: int, tags: [str] }
fn bump(p: P) { p.x = p.x + 1; push(p.tags, "b"); }
fn main() { let p = P { tags: ["a"], x: 1 }; let q = p; bump(q); print(p.x, p.tags, p); }
"#;
    ok(src, "2 [\"a\", \"b\"] P {x: 2, tags: [\"a\", \"b\"]}\n");
}

#[test]
fn recursion_and_mutual_recursion() {
    let src = r#"
fn even(n: int) -> bool { if n == 0 { return true; } return odd(n - 1); }
fn odd(n: int) -> bool { if n == 0 { return false; } return even(n - 1); }
fn fact(n: int) -> int { if n <= 1 { return 1; } return n * fact(n - 1); }
fn main() { print(even(10), odd(7), fact(20)); }
"#;
    ok(src, "true true 2432902008176640000\n");
}

#[test]
fn shadowing_in_inner_block() {
    ok(
        &main_body("let x = 1; if true { let x = x + 10; print(x); } print(x);"),
        "11\n1\n",
    );
}

#[test]
fn assert_passes_and_comments() {
    ok(
        "# comment\nfn main() { assert(1 + 1 == 2, \"math\"); # trailing\n print(\"ok\"); }",
        "ok\n",
    );
}

#[test]
fn float_formatting() {
    ok(
        &main_body("print(0.1 + 0.2, 1000.0, 100.0, -0.5, 10.0 / 4.0, 7.5 % 2.0);"),
        "0.30000000000000004 1000.0 100.0 -0.5 2.5 1.5\n",
    );
}

// ---- runtime errors ----

#[test]
fn runtime_index_out_of_bounds() {
    fails(
        run("fn main() {\n  let xs = [1, 2];\n  print(xs[2]);\n}"),
        "runtime error at 3:11: index 2 out of bounds for list of length 2",
    );
}

#[test]
fn runtime_negative_index() {
    fails(
        run("fn main() {\n  let xs = [1];\n  xs[-1] = 3;\n}"),
        "runtime error at 3:5: index -1 out of bounds for list of length 1",
    );
}

#[test]
fn runtime_division_by_zero() {
    fails(
        run("fn main() {\n  let z = 0;\n  print(1 % 1, 5 / z);\n}"),
        "runtime error at 3:18: division by zero",
    );
}

#[test]
fn runtime_integer_overflow() {
    fails(
        run("fn main() {\n  let x = 9223372036854775807;\n  let y = x + 1;\n}"),
        "runtime error at 3:13: integer overflow in '+'",
    );
}

#[test]
fn runtime_overflow_mul_and_neg() {
    fails(
        run("fn main() { let x = 4611686018427387904; print(x * 2); }"),
        "runtime error at 1:50: integer overflow in '*'",
    );
}

#[test]
fn runtime_pop_empty() {
    fails(
        run("fn main() {\n  let xs: [int] = [];\n  pop(xs);\n}"),
        "runtime error at 3:3: pop from empty list",
    );
}

#[test]
fn runtime_assert_with_and_without_message() {
    fails(
        run("fn main() {\n  assert(1 > 2, \"bad \" + str(7));\n}"),
        "runtime error at 2:3: assertion failed: bad 7",
    );
    fails(
        run("fn main() { assert(false); }"),
        "runtime error at 1:13: assertion failed",
    );
}

#[test]
fn runtime_bad_int_conversion() {
    fails(
        run("fn main() {\n print(int(\"12x\"));\n}"),
        "runtime error at 2:8: invalid int literal",
    );
}

#[test]
fn output_before_error_is_flushed() {
    let o = run("fn main() { print(\"before\"); let xs: [int] = []; print(xs[0]); }");
    assert_eq!(o.stdout, "before\n");
    assert_eq!(o.code, 1);
}

// ---- limits ----

#[test]
fn infinite_loop_hits_step_limit() {
    let o = limits(
        "run",
        "fn main() {\n  let i = 0;\n  while true {\n    i = i + 1;\n  }\n}",
        &["--max-steps", "100000"],
    );
    fails(o, "runtime error at 3:9: step limit exceeded");
}

#[test]
fn default_step_limit_stops_infinite_loop() {
    let o = run("fn main() { while true { } }");
    fails(o, "runtime error at 1:19: step limit exceeded");
}

#[test]
fn infinite_recursion_hits_depth_limit() {
    let o = run("fn f(n: int) -> int {\n  return f(n + 1);\n}\nfn main() { print(f(0)); }");
    fails(o, "runtime error at 1:1: call depth limit exceeded");
}

#[test]
fn max_depth_flag() {
    let src = "fn d(n: int) -> int { if n == 0 { return 0; } return 1 + d(n - 1); }\nfn main() { print(d(50)); }";
    assert_eq!(limits("run", src, &["--max-depth", "100"]).stdout, "50\n");
    fails(
        limits("run", src, &["--max-depth", "20"]),
        "runtime error at 1:1: call depth limit exceeded",
    );
}

#[test]
fn step_limit_counts_calls() {
    let src = "fn g() { }\nfn main() { g(); g(); g(); }";
    assert_eq!(limits("run", src, &["--max-steps", "4"]).code, 0);
    fails(
        limits("run", src, &["--max-steps", "3"]),
        "runtime error at 1:1: step limit exceeded",
    );
}

#[test]
fn unchecked_removes_limits_but_keeps_bounds() {
    let src = "fn main() { let i = 0; while i < 1000 { i = i + 1; } print(i); let x = 9223372036854775807; print(x + 1); }";
    assert_eq!(
        limits("run", src, &["--unchecked", "--max-steps", "10"]).stdout,
        "1000\n-9223372036854775808\n"
    );
    fails(
        limits(
            "run",
            "fn main() { let xs = [1]; print(xs[5]); }",
            &["--unchecked"],
        ),
        "runtime error at 1:35: index 5 out of bounds for list of length 1",
    );
}

// ---- compile-time errors ----

#[test]
fn check_reports_all_errors() {
    let src = r#"
fn f(a: int) -> int {
    return a + 1.0;
}
fn main() {
    let x = y;
    let s: str = 3;
    z = 1;
    f("a");
    break;
    let e = [];
    print(undefined_fn(1));
}
"#;
    let o = limits("check", src, &[]);
    fails(
        o,
        "check error at 3:14: cannot apply '+' to int and float (use float(x) or int(x) to convert)\n\
         check error at 6:13: undefined variable 'y'\n\
         check error at 7:18: value of 's' must be str, got int\n\
         check error at 8:5: assignment to undeclared variable 'z'\n\
         check error at 9:7: argument 1 of 'f' must be int, got str\n\
         check error at 10:5: 'break' outside of a loop\n\
         check error at 11:13: cannot infer the type of an empty list; add an annotation like 'let xs: [int] = [];'\n\
         check error at 12:11: undefined function 'undefined_fn'",
    );
}

#[test]
fn check_struct_and_return_errors() {
    let src = "struct P { x: int }\nfn g() -> int { if true { return 1; } }\nfn main() { let p = P { y: 1 }; print(p.z); let q: Q = p; }";
    fails(
        limits("check", src, &[]),
        "check error at 2:1: function 'g' may finish without returning a int\n\
         check error at 3:21: field 'x' missing in 'P' literal\n\
         check error at 3:28: struct 'P' has no field 'y'\n\
         check error at 3:40: struct 'P' has no field 'z'\n\
         check error at 3:52: unknown type 'Q'",
    );
}

#[test]
fn check_missing_main() {
    fails(
        limits("check", "pub fn f() { }", &[]),
        "check error at 1:1: missing 'fn main()'",
    );
}

#[test]
fn lex_and_parse_errors() {
    fails(
        limits("check", "fn main() { let s = \"abc; }", &[]),
        "lex error at 1:21: unterminated string",
    );
    fails(
        limits("check", "fn main() { let x = 1 }", &[]),
        "parse error at 1:23: expected ';', found '}'",
    );
    fails(
        limits("check", "fn main() { print(1 < 2 < 3); }", &[]),
        "parse error at 1:25: comparison operators cannot be chained, found '<'",
    );
}

#[test]
fn build_emits_binary_and_emit_c() {
    let dir = std::env::temp_dir().join(format!("limits-build-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("p.lim");
    std::fs::write(&file, "fn main() { print(\"built\"); }").unwrap();
    let bin = dir.join("p.bin");
    let st = Command::new(env!("CARGO_BIN_EXE_limits"))
        .arg("build")
        .arg(&file)
        .arg("-o")
        .arg(&bin)
        .status()
        .unwrap();
    assert!(st.success());
    let o = Command::new(&bin).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout), "built\n");
    let c = Command::new(env!("CARGO_BIN_EXE_limits"))
        .arg("build")
        .arg(&file)
        .arg("--emit-c")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&c.stdout).contains("int main(void)"));
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
        let o = Command::new(env!("CARGO_BIN_EXE_limits"))
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
    let src = "fn g() { for i in 0..2 { } }\nfn main() { for i in 0..5 { g(); } print(\"done\"); }";
    assert_eq!(limits("run", src, &["--max-steps", "21"]).stdout, "done\n");
    fails(
        limits("run", src, &["--max-steps", "20"]),
        "runtime error at 1:19: step limit exceeded",
    );
}

#[test]
fn sqrt_builtin() {
    ok(
        &main_body("print(sqrt(16.0), sqrt(2.0));"),
        "4.0 1.4142135623730951\n",
    );
    let o = limits("check", &main_body("print(sqrt(4));"), &[]);
    fails(o, "check error at 2:12: 'sqrt' expects a float, got int");
}

/// Large programs are split into several C files compiled in parallel; the
/// result and the limits must be the same as for a single file.
#[test]
fn large_program_split_into_parallel_c_files() {
    let mut src = String::from("fn f0(n: int) -> int { return n; }\n");
    for i in 1..400 {
        src += &format!(
            "fn f{i}(n: int) -> int {{ let s = 0; for k in 0..3 {{ s = s + k; }} return f{}(n + 1) + s - 3; }}\n",
            i - 1
        );
    }
    src += "fn main() { print(f399(0)); }\n";
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
fn printing_cyclic_value_is_a_runtime_error() {
    for flags in [&[][..], &["--unchecked"][..]] {
        let o = limits(
            "run",
            "struct N { v: int, next: [N] }\nfn main() { let n = N { v: 1, next: [] }; push(n.next, n); print(n); }",
            flags,
        );
        fails(
            o,
            "runtime error at 2:60: value nested too deeply to format (cyclic?)",
        );
    }
}

#[test]
fn str_of_deep_chain_is_a_runtime_error() {
    let o = run(&main_body(
        "let h = N { v: 0, next: [] };\nfor i in 0..300000 { h = N { v: i, next: [h] }; }\nprint(len(str(h)));",
    )
    .replace("fn main", "struct N { v: int, next: [N] }\nfn main"));
    fails(
        o,
        "runtime error at 5:11: value nested too deeply to format (cyclic?)",
    );
}

#[test]
fn int_min_div_minus_one_names_division() {
    fails(
        run("fn main() { let m = -9223372036854775807 - 1; print(m / -1); }"),
        "runtime error at 1:55: integer overflow in '/'",
    );
}

#[test]
fn int_min_literal() {
    ok(
        "fn main() { let x = -9223372036854775808; print(x, x + 1); }",
        "-9223372036854775808 -9223372036854775807\n",
    );
    fails(
        limits("check", "fn main() { let x = 9223372036854775808; }", &[]),
        "lex error at 1:21: integer literal '9223372036854775808' out of range",
    );
}

#[test]
fn self_containing_struct_rejected() {
    fails(
        limits("check", "struct S { s: S } fn main() { print(1); }", &[]),
        "check error at 1:1: struct 'S' contains itself (only through a list can a struct refer to itself), so it can never be constructed",
    );
}

#[test]
fn struct_named_like_builtin_type_rejected() {
    fails(
        limits("check", "struct int { x: int } fn main() { }", &[]),
        "check error at 1:1: 'int' is a builtin type name",
    );
}

#[test]
fn float_specials_lowercase() {
    ok(
        "fn main() { print(1.0/0.0, -1.0/0.0, 0.0/0.0, str(0.0/0.0)); }",
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
        let o = run(&format!("fn main() {{ print({}); }}", src));
        fails(o, &format!("runtime error at 1:19: {}", msg));
    }
    ok(
        r#"fn main() { print(int("-12"), int("+7"), float("1.5e3"), float("-.5")); }"#,
        "-12 7 1500.0 -0.5\n",
    );
}

#[test]
fn big_frames_hit_depth_limit_not_segfault() {
    let n = 300;
    let lets: String = (0..n)
        .map(|i| format!("let a{} = n * {};", i, i + 1))
        .collect();
    let sum: Vec<String> = (0..n).map(|i| format!("a{}", i)).collect();
    let src = format!(
        "fn f(n: int) -> int {{ {} if n == 0 {{ return 0; }} let r = f(n - 1); return r + {}; }}\nfn main() {{ print(f(9990)); }}\n",
        lets,
        sum.join(" + ")
    );
    fails(run(&src), "runtime error at 1:1: call depth limit exceeded");
}

#[test]
fn builtins_charge_steps() {
    let src = main_body(
        "let s = \"a\";\nfor i in 0..28 { s = s + s; }\nlet n = 0;\nfor k in 0..300 { n = n + len(s); }\nprint(n);",
    );
    fails(
        limits("run", &src, &["--max-steps", "1000"]),
        "runtime error at 3:24: step limit exceeded",
    );
}

#[test]
fn memory_budget() {
    let src = main_body("let s = \"xxxxxxxxxxxxxxxx\";\nwhile true {\n  s = s + s;\n}");
    for flags in [&[][..], &["--unchecked"][..]] {
        fails(
            limits("run", &src, flags),
            "runtime error at 4:9: memory limit exceeded",
        );
    }
    let o = limits(
        "run",
        &main_body("let xs = [0];\nwhile true { push(xs, 1); }"),
        &["--max-mem", "8"],
    );
    fails(o, "runtime error at 3:14: memory limit exceeded");
}

#[test]
fn compiler_rejects_too_deep_nesting() {
    let chain = vec!["1"; 20000].join(" + ");
    let o = limits("check", &format!("fn main() {{ let x = {}; }}", chain), &[]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("parse error") && o.stderr.contains("nesting too deep"));
    let parens = format!(
        "fn main() {{ let x = {}1{}; }}",
        "(".repeat(100000),
        ")".repeat(100000)
    );
    let o = limits("check", &parens, &[]);
    assert!(o.stderr.contains("nesting too deep"), "{}", o.stderr);
    let ifs = format!(
        "fn main() {{ {}{} }}",
        "if true {".repeat(50000),
        "}".repeat(50000)
    );
    let o = limits("check", &ifs, &[]);
    assert!(o.stderr.contains("nesting too deep"), "{}", o.stderr);
    ok(
        &main_body(&format!("print({});", vec!["1"; 2500].join(" + "))),
        "2500\n",
    );
}

#[test]
fn run_reports_signal_death() {
    // --unchecked drops the depth/stack check, so runaway recursion dies by SIGSEGV.
    let o = limits(
        "run",
        "fn f(xs: [int]) { push(xs, 1); f(xs); print(len(xs)); }\nfn main() { f([0]); }",
        &["--unchecked"],
    );
    assert_eq!(o.code, 139);
    assert_eq!(o.stderr.trim_end(), "limits: program killed by signal 11");
}
