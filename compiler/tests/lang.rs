use limits::error::Kind;
use limits::interp::{Interpreter, Limits};
use limits::{lexer, parser, run};

fn out(src: &str) -> String {
    let mut buf = Vec::new();
    run(src, &mut buf).unwrap_or_else(|e| panic!("{}", e));
    String::from_utf8(buf).unwrap()
}

fn err(src: &str) -> limits::error::LimError {
    let mut buf = Vec::new();
    run(src, &mut buf).expect_err("expected an error")
}

#[test]
fn arithmetic_and_precedence() {
    assert_eq!(
        out("print(1 + 2 * 3, (1 + 2) * 3, 7 / 2, 7 % 3, -2 * 3);"),
        "7 9 3 1 -6\n"
    );
    assert_eq!(out("print(1 + 0.5, 3.0);"), "1.5 3.0\n");
}

#[test]
fn strings_lists_maps() {
    assert_eq!(
        out(r#"print("a" + "b", len("héllo"), "abc"[-1]);"#),
        "ab 5 c\n"
    );
    assert_eq!(
        out("let l = [1, 2]; push(l, 3); l[0] = 9; print(l, len(l), pop(l));"),
        "[9, 2] 3 3\n"
    );
    assert_eq!(
        out(r#"let m = {"a": 1}; m["b"] = "x"; print(m, m["zz"], keys(m));"#),
        "{\"a\": 1, \"b\": \"x\"} nil [\"a\", \"b\"]\n"
    );
}

#[test]
fn control_flow() {
    let src = "
        let s = 0;
        for i in 0..10 {
            if i == 2 { continue; }
            if i == 5 { break; }
            s = s + i;
        }
        let w = 0;
        while w < 3 { w = w + 1; }
        if false { print(0); } else if s == 8 { print(s, w); } else { print(-1); }
    ";
    assert_eq!(out(src), "8 3\n");
}

#[test]
fn functions_closures_recursion() {
    let src = "
        fn make_adder(n) { fn add(x) { return x + n; } return add; }
        let add2 = make_adder(2);
        fn fact(n) { if n <= 1 { return 1; } return n * fact(n - 1); }
        print(add2(5), fact(10));
    ";
    assert_eq!(out(src), "7 3628800\n");
}

#[test]
fn logic_short_circuits() {
    assert_eq!(
        out("print(false and undefined_thing, true or undefined_thing, not nil);"),
        "false true true\n"
    );
}

#[test]
fn block_scoping() {
    assert_eq!(out("let x = 1; if true { let x = 2; } print(x);"), "1\n");
    assert_eq!(out("let x = 1; if true { x = 2; } print(x);"), "2\n");
}

#[test]
fn runtime_errors_have_locations() {
    let e = err("let x = 1;\nprint(y);");
    assert_eq!(e.kind, Kind::Runtime);
    assert_eq!((e.span.line, e.span.col), (2, 7));
    assert!(err("x = 1;").msg.contains("undeclared"));
    assert!(err("print(1 / 0);").msg.contains("division by zero"));
    assert!(err("print([1][5]);").msg.contains("out of bounds"));
    assert!(err("print(9223372036854775807 + 1);")
        .msg
        .contains("overflow"));
    assert!(err("print(1 + \"a\");")
        .msg
        .contains("unsupported operands"));
    assert!(err("assert(false, \"boom\");").msg.contains("boom"));
    assert!(err("break;").msg.contains("outside of a loop"));
    let e = err("let m = {1: 2};");
    assert!(e.msg.contains("map keys must be strings"));
    assert_eq!((e.span.line, e.span.col), (1, 9));
}

#[test]
fn parse_and_lex_errors() {
    assert_eq!(err("let = 3;").kind, Kind::Parse);
    assert_eq!(err("print(1)").kind, Kind::Parse);
    assert_eq!(err("print(1 < 2 < 3);").kind, Kind::Parse);
    assert_eq!(err("1 + 2 = 3;").kind, Kind::Parse);
    assert_eq!(err("let s = \"open;").kind, Kind::Lex);
    assert_eq!(err("let a = 1 @ 2;").kind, Kind::Lex);
}

fn run_limited(src: &str, limits: Limits) -> Result<(), limits::error::LimError> {
    let prog = parser::parse(lexer::lex(src)?)?;
    let mut buf = Vec::new();
    Interpreter::with_limits(&mut buf, limits).run(&prog)
}

#[test]
fn step_limit_stops_runaway_loops() {
    let e = run_limited(
        "while true { }",
        Limits {
            max_steps: 1000,
            max_depth: 64,
        },
    )
    .unwrap_err();
    assert!(e.msg.contains("step limit"), "{}", e);
}

#[test]
fn depth_limit_stops_runaway_recursion() {
    let e = run_limited(
        "fn f(n) { return f(n + 1); } f(0);",
        Limits {
            max_steps: 1_000_000,
            max_depth: 50,
        },
    )
    .unwrap_err();
    assert!(e.msg.contains("depth limit"), "{}", e);
}

#[test]
fn examples_run() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples");
    assert_eq!(
        out(&std::fs::read_to_string(format!("{}/hello.lim", dir)).unwrap()),
        "hello, limits\n"
    );
    assert_eq!(
        out(&std::fs::read_to_string(format!("{}/fib.lim", dir)).unwrap()),
        "fib: [0, 1, 1, 2, 3, 5, 8, 13, 21, 34]\n"
    );
    assert_eq!(
        out(&std::fs::read_to_string(format!("{}/agent_task.lim", dir)).unwrap()),
        "chosen: search\n"
    );
}
