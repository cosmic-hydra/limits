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
         check error at 8:5: assignment to undeclared variable 'z'\n  hint: declare it first: let z = ...;\n\
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

#[test]
fn packed_lists_of_every_element_type() {
    ok(
        "struct P { x: int }\nfn main() {\n\
         let bs = [true, false]; push(bs, true); bs[1] = true; print(bs, pop(bs), len(bs));\n\
         let fs = [1.5]; push(fs, 2.0); fs[0] = fs[1] * 2.0; print(fs);\n\
         let ss: [str] = []; push(ss, \"a\"); push(ss, str(bs)); print(ss, ss[1]);\n\
         let ps = [P {x: 1}]; push(ps, P {x: 2}); ps[0].x = 7;\n\
         let t = 0; for p in ps { t = t + p.x; } print(ps, t, pop(ps).x);\n\
         let nest = [[1, 2], [3]]; push(nest[1], 4); print(nest, nest[1][1]);\n\
         let big: [bool] = []; for i in 0..1000 { push(big, i % 3 == 0); }\n\
         let c = 0; for b in big { if b { c = c + 1; } } print(c, big[999]);\n}",
        "[true, true, true] true 2\n[4.0, 2.0]\n[\"a\", \"[true, true]\"] [true, true]\n[P {x: 7}, P {x: 2}] 9 2\n[[1, 2], [3, 4]] 4\n334 true\n",
    );
}

#[test]
fn json_diagnostics() {
    let o = limits(
        "check",
        "fn main() {\n  let total = 1;\n  print(totl);\n}\n",
        &["--json"],
    );
    assert_eq!(o.code, 1);
    let l = o.stderr.trim_end();
    assert!(
        l.starts_with(r#"{"kind":"check","code":"E0201","file":""#),
        "{}",
        l
    );
    assert!(l.ends_with(r#"t.lim","line":3,"col":9,"msg":"undefined variable 'totl'","hint":"did you mean 'total'?"}"#), "{}", l);
    let o = limits(
        "run",
        "fn main() {\n  assert(false, \"a \\\"q\\\"\");\n}\n",
        &["--json"],
    );
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains(r#""code":"E0506","#), "{}", o.stderr);
    assert!(
        o.stderr
            .trim_end()
            .ends_with(r#""line":2,"col":3,"msg":"assertion failed: a \"q\"","hint":""}"#),
        "{}",
        o.stderr
    );
    let o = limits("check", "fn main() { let x = 1 +; }", &["--json"]);
    assert!(
        o.stderr.starts_with(r#"{"kind":"parse","code":"E0100","#),
        "{}",
        o.stderr
    );
}

#[test]
fn inline_tests_run_isolated() {
    let src = r#"
fn f(n: int) -> int { return n + 1; }
test "passes" { assert(f(1) == 2); }
test "loops forever" { while true { } }
test "bad index" { let xs = [1]; print(xs[5]); }
test "still runs" { assert(f(0) == 1, "f"); }
fn main() { print("main"); }
"#;
    let o = limits("test", src, &["--max-steps", "1000"]);
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert_eq!(
        o.stdout,
        "ok passes\nFAIL loops forever: runtime error at 4:30: step limit exceeded\n\
         FAIL bad index: runtime error at 5:42: index 5 out of bounds for list of length 1\n\
         ok still runs\n2 passed, 2 failed\n"
    );
    // Tests are type checked but not part of build/run output.
    ok(src, "main\n");
    assert_eq!(limits("test", "test \"t\" { }", &[]).code, 0);
}

#[test]
fn contracts() {
    let src = r#"
fn isqrt(n: int) -> int
  requires n >= 0;
  ensures result * result <= n;
  ensures (result + 1) * (result + 1) > n;
{
  let r = 0;
  while (r + 1) * (r + 1) <= n { r = r + 1; }
  if n == 99 { return r + 1; }
  return r;
}
fn main() { print(isqrt(17)); print(isqrt(ARG)); }
"#;
    let o = run(&src.replace("ARG", "-1"));
    assert_eq!(o.stdout, "4\n");
    fails(o, "runtime error at 3:14: requires failed in fn isqrt");
    fails(
        run(&src.replace("ARG", "99")),
        "runtime error at 4:27: ensures failed in fn isqrt",
    );
    let o = limits("run", &src.replace("ARG", "-1"), &["--unchecked"]);
    assert_eq!((o.code, o.stdout.as_str()), (0, "4\n0\n"));
    fails(
        limits("check", "fn f() ensures result; { }\nfn main() { }", &[]),
        "check error at 1:16: undefined variable 'result'\n  \
         hint: 'result' only exists in 'ensures' of a fn with a return type",
    );
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
    let mut ch = Command::new(env!("CARGO_BIN_EXE_limits"))
        .arg("run")
        .arg("t.lim")
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
    let src = r#"uses fs.read, fs.write, env, time, args, stdin;
fn main() {
  write_file("sub/o.txt", read_file("in.txt") + "!");
  print(read_file("sub/o.txt"), env("LIMITS_NO_SUCH_VAR") == "", now_ms() > 0, args());
  print(input_line(), input_line(), input_line() == "");
}
"#;
    let (o, dir) = in_dir(src, &["--", "a", "b c"], &[("in.txt", "hi")], "one\ntwo");
    let _ = std::fs::remove_dir_all(dir);
    assert_eq!((o.code, o.stderr.as_str()), (0, ""));
    assert_eq!(o.stdout, "hi! true true [\"a\", \"b c\"]\none two true\n");
}

#[test]
fn io_builtins_need_declared_capabilities() {
    fails(
        limits("check", "uses net;\nfn main() { print(read_file(\"x\")); }\n", &[]),
        "check error at 1:6: unknown capability 'net'\n  hint: capabilities are: fs.read, fs.write, env, time, args, stdin\ncheck error at 2:19: 'read_file' needs capability 'fs.read'\n  hint: add 'uses fs.read;' at the top of this file",
    );
}

#[test]
fn fs_access_is_sandboxed() {
    let src = "uses fs.read, fs.write, args;\nfn main() { let p = args()[0]; if p == \"w\" { write_file(\"../x\", \"\"); } print(len(read_file(p))); }\n";
    let cases: &[(&[&str], &str)] = &[
        (&["--", "../t.lim"], "runtime error at 2:82: access denied: '../t.lim' is outside the allowed fs.read paths (see --allow)"),
        (&["--", "sub/../../t.lim"], "runtime error at 2:82: access denied: 'sub/../../t.lim' is outside the allowed fs.read paths (see --allow)"),
        (&["--", "w"], "runtime error at 2:46: access denied: '../x' is outside the allowed fs.write paths (see --allow)"),
        (&["--", "nope"], "runtime error at 2:82: cannot read file 'nope': No such file or directory"),
        (&["--", "sub"], "runtime error at 2:82: cannot read file 'sub': not a regular file"),
        (&["--allow", "fs.read=sub", "--", "t.lim"], "runtime error at 2:82: access denied: 't.lim' is outside the allowed fs.read paths (see --allow)"),
    ];
    for (flags, want) in cases {
        let (o, dir) = in_dir(src, flags, &[], "");
        let _ = std::fs::remove_dir_all(dir);
        fails(o, want);
    }
    // A symlink inside the sandbox pointing out of it is an escape.
    let (_, dir) = in_dir(src, &["--", "x"], &[], "");
    std::os::unix::fs::symlink("/etc/hostname", dir.join("link")).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_limits"))
        .args(["run", "t.lim", "--", "link"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        String::from_utf8_lossy(&o.stderr).trim_end(),
        "runtime error at 2:82: access denied: 'link' is outside the allowed fs.read paths (see --allow)"
    );
}

#[test]
fn file_reads_count_steps_and_memory() {
    let src = "uses fs.read;\nfn main() { print(len(read_file(\"big\"))); }\n";
    let big = "x".repeat(1 << 20);
    let (o, dir) = in_dir(src, &["--max-steps", "1000"], &[("big", &big)], "");
    let _ = std::fs::remove_dir_all(dir);
    fails(o, "runtime error at 2:23: step limit exceeded");
    let (o, dir) = in_dir(src, &["--max-mem", "1"], &[("big", &big)], "");
    let _ = std::fs::remove_dir_all(dir);
    fails(o, "runtime error at 2:23: memory limit exceeded");
}

#[test]
fn split_join_sort() {
    ok(
        &main_body(
            r#"let p = split("b,a,,c", ",");
print(len(p), p, split("", ","), split("hé!", ""), split("x", ""));
print(join(p, "+"), join(split("a b", " "), ""));
let e: [str] = [];
print(join(e, ","), len(join(e, ",")));
sort(p);
print(p);
let xs = [3, -1, 2, 2];
sort(xs);
let fs = [2.5, -0.5, 1.0];
sort(fs);
print(xs, fs);"#,
        ),
        "4 [\"b\", \"a\", \"\", \"c\"] [\"\"] [\"h\", \"é\", \"!\"] [\"x\"]\nb+a++c ab\n 0\n[\"\", \"a\", \"b\", \"c\"]\n[-1, 2, 2, 3] [-0.5, 1.0, 2.5]\n",
    );
    let o = run(&main_body(
        "sort([true]);\nlet n = join([1], \",\");\nsplit(1, 2);",
    ));
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
    let big = main_body("let xs: [int] = [];\nfor i in 0..100000 { push(xs, 100000 - i); }\nsort(xs);\nprint(xs[0]);");
    let o = limits("run", &big, &["--max-steps", "1000000"]);
    assert!(o.stderr.contains("step limit exceeded"), "{}", o.stderr);
}

/// Run `limits <args>` in `dir` and return (stdout, stderr, exit code).
fn cmd_in(dir: &std::path::Path, args: &[&str]) -> (String, String, i32) {
    let o = Command::new(env!("CARGO_BIN_EXE_limits"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (s(&o.stdout), s(&o.stderr), o.status.code().unwrap_or(-1))
}

#[test]
fn fifo_is_rejected_without_blocking() {
    let src = "uses fs.read, fs.write, args;\nfn main() { if len(args()) > 0 { write_file(\"fifo\", \"x\"); } print(read_file(\"fifo\")); }\n";
    let (_, dir) = in_dir("fn main() {}", &[], &[], "");
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
        "runtime error at 2:67: cannot read file 'fifo': not a regular file"
    );
    assert_eq!(
        w.trim_end(),
        "runtime error at 2:34: cannot write file 'fifo': not a regular file"
    );
}

#[test]
fn input_line_keeps_nul_bytes_and_charges_steps() {
    let src = "uses stdin;\nfn main() { let s = input_line(); print(len(s), s == \"x\" + split(\"a\", \"a\")[0]); }\n";
    let (o, dir) = in_dir(src, &[], &[], "x\0yz\n");
    let _ = std::fs::remove_dir_all(dir);
    assert_eq!(o.stdout, "4 false\n");
    let zeros = "\0".repeat(1 << 20);
    let (o, dir) = in_dir(src, &["--max-steps", "100"], &[], &zeros);
    let _ = std::fs::remove_dir_all(dir);
    fails(o, "runtime error at 2:21: step limit exceeded");
}

#[test]
fn test_names_are_escaped() {
    let src = "test \"x\\nok 5 passed, 0 failed\\nPASS \\\"q\" { assert(false); }\n";
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
    let src =
        "uses fs.read;\nfn main() { let p = read_file(\"bin\"); print(read_file(\"../\" + p)); }\n";
    let (_, dir) = in_dir(src, &[], &[], "");
    std::fs::write(dir.join("bin"), b"a\xffb\xe2\x82\xac").unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_limits"))
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
    std::fs::write(dir.join("u.lim"), b"fn main() {\n  print(\"\xff\"); }\n").unwrap();
    let (_, e, code) = cmd_in(&dir, &["check", "u.lim", "--json"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(code, 1);
    assert_eq!(
        e.trim_end(),
        r#"{"kind":"lex","code":"E0005","file":"u.lim","line":2,"col":10,"msg":"invalid UTF-8 in source file","hint":""}"#
    );
}

#[test]
fn small_memory_budget_charges_bytes_not_chunks() {
    let src =
        "struct P { x: int }\nfn main() { let p = P { x: 1 }; let xs = [[1]]; print(p, xs); }\n";
    let o = limits("run", src, &["--max-mem", "1"]);
    assert_eq!(
        (o.stdout.as_str(), o.code),
        ("P {x: 1} [[1]]\n", 0),
        "{}",
        o.stderr
    );
    let grow = main_body("let xs = [\"a\"];\nwhile true { push(xs, xs[0] + \"b\"); }");
    let o = limits("run", &grow, &["--max-mem", "1"]);
    fails(o, "runtime error at 3:29: memory limit exceeded");
}

#[test]
fn floats_print_without_exponents() {
    ok(
        &main_body("let f = 100000000000000000000.0;\nprint(f, [f, 0.000001], str(f) == \"100000000000000000000.0\", float(\"1.25e-7\"), -0.00012);"),
        "100000000000000000000.0 [100000000000000000000.0, 0.000001] true 0.000000125 -0.00012\n",
    );
}

#[test]
fn print_writes_nothing_when_an_argument_fails() {
    let o = run("fn f() -> int { print(\"inner\"); return 3; }\nfn main() { print(1, f()); print(len(\"\"), 1, int(\"\"), 2); }\n");
    assert_eq!(o.stdout, "inner\n1 3\n");
    assert_eq!(
        o.stderr.trim_end(),
        "runtime error at 2:46: invalid int literal"
    );
}

#[test]
fn test_runner_survives_closed_stdout_and_timeouts() {
    let dir = std::env::temp_dir().join(format!("limits-tt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let many: String = (0..20).map(|i| format!("test \"t{}\" {{}}\n", i)).collect();
    std::fs::write(dir.join("t1.lim"), many).unwrap();
    let t2 = "fn r(n: int) -> int { return r(n + 1) + 1; }\ntest \"rec\" { print(r(0)); }\ntest \"after\" { }\n";
    std::fs::write(dir.join("t2.lim"), t2).unwrap();
    let mut ch = Command::new(env!("CARGO_BIN_EXE_limits"))
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
