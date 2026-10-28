//! Tree continuation regressions at collection, return and invocation boundaries.
use limits::error::Kind;
use limits::interp::{Interpreter, Limits};
use limits::{ast::Program, lexer, parser};

fn parse(source: &str) -> Program {
    parser::parse(lexer::lex(source).unwrap()).unwrap()
}

#[test]
fn pending_values_and_iterator_closures_survive_collection() {
    let source = r#"
        fn churn() {
            for i in 0..1300 { let garbage = [i]; }
            return 7;
        }
        fn make(n) {
            let cell = [n];
            fn read(extra) { return cell[0] + extra; }
            return read;
        }
        print([
            make(10)(churn()),
            {"saved": [3], "after": churn()},
            [make(20), churn()][0](2)
        ]);
        for read in [make(30), make(40)] { print(read(churn())); }
    "#;
    let mut out = Vec::new();
    Interpreter::new(&mut out).run(&parse(source)).unwrap();
    assert_eq!(out, b"[17, {\"after\": 7, \"saved\": [3]}, 22]\n37\n47\n");
}

#[test]
fn nested_loop_returns_preserve_pending_caller_values() {
    let source = r#"
        fn select(n) {
            for outer in [0, 1, 2] {
                let inner = 0;
                while inner < 3 {
                    inner = inner + 1;
                    if outer == n {
                        if inner == 2 { return outer * 10 + inner; }
                        continue;
                    }
                    if inner == 2 { break; }
                }
            }
        }
        fn empty() {
            for x in [1, 2] { if x == 1 { continue; } break; }
        }
        print([100, select(1), empty(), select(9), 200], select(2) + select(0));
    "#;
    let mut out = Vec::new();
    Interpreter::new(&mut out).run(&parse(source)).unwrap();
    assert_eq!(out, b"[100, 12, nil, nil, 200] 24\n");
}

#[test]
fn failed_call_restores_depth_and_preserves_exported_closure_for_reuse() {
    let failed = parse(
        r#"
        let saved = nil;
        fn fail() {
            let cell = [42];
            fn read() { return cell[0]; }
            saved = read;
            assert(false, "first invocation");
        }
        fail();
    "#,
    );
    let later = parse("print(saved());");
    let mut out = Vec::new();
    {
        let mut interpreter = Interpreter::with_limits(
            &mut out,
            Limits {
                max_steps: 100,
                max_depth: 1,
            },
        );
        let error = interpreter.run(&failed).unwrap_err();
        assert_eq!(error.kind, Kind::Runtime);
        assert_eq!(error.msg, "first invocation");
        interpreter.run(&later).unwrap();
        interpreter.run(&later).unwrap();
    }
    assert_eq!(out, b"42\n42\n");
}

#[test]
fn direct_tree_reuse_preserves_the_cumulative_step_budget() {
    let mut out = Vec::new();
    {
        let mut interpreter = Interpreter::with_limits(
            &mut out,
            Limits {
                max_steps: 4,
                max_depth: 1,
            },
        );
        interpreter.run(&parse("let n = 1;")).unwrap();
        interpreter.run(&parse("n = n + 1; print(n);")).unwrap();
        let error = interpreter.run(&parse("n = n + 1;")).unwrap_err();
        assert_eq!(error.kind, Kind::Runtime);
        assert_eq!(error.msg, "step limit exceeded (4 steps)");
        assert_eq!((error.span.line, error.span.col), (1, 1));
    }
    assert_eq!(out, b"2\n");
}
