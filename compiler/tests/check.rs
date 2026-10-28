use limits::error::Kind;
use limits::{lexer, parser, resolve};

fn check(src: &str) -> Vec<String> {
    let prog = parser::parse(lexer::lex(src).unwrap()).unwrap();
    resolve::check(&prog)
        .into_iter()
        .map(|e| {
            assert_eq!(e.kind, Kind::Check);
            format!("{}:{} {}", e.span.line, e.span.col, e.msg)
        })
        .collect()
}

#[test]
fn clean_programs_pass() {
    let src = "
        fn even(n) { if n == 0 { return true; } return odd(n - 1); }
        fn odd(n) { if n == 0 { return false; } return even(n - 1); }
        let total = 0;
        for i in 0..3 { total = total + i; if i > 1 { break; } }
        while false { continue; }
        print(even(4), len([total]));
    ";
    assert_eq!(check(src), Vec::<String>::new());
    for f in ["hello", "fib", "agent_task", "runaway"] {
        let p = format!("{}/../examples/{}.lim", env!("CARGO_MANIFEST_DIR"), f);
        assert_eq!(
            check(&std::fs::read_to_string(p).unwrap()),
            Vec::<String>::new(),
            "{}",
            f
        );
    }
}

#[test]
fn reports_every_problem_at_once() {
    let src = "print(totl);\ncount = 1;\nbreak;\nfn f(a, a) { continue; }\nlet m = {\"k\": undefined_value};";
    assert_eq!(
        check(src),
        vec![
            "1:7 undefined variable 'totl'",
            "2:1 assignment to undeclared variable 'count' (use 'let')",
            "3:1 'break' outside of a loop",
            "4:1 duplicate parameter 'a' in fn f",
            "4:14 'continue' outside of a loop",
            "5:15 undefined variable 'undefined_value'",
        ]
    );
}

#[test]
fn scoping_rules() {
    // Loop variables and params don't leak; inner lets don't leak.
    assert_eq!(
        check("for i in [1] { } print(i);"),
        vec!["1:24 undefined variable 'i'"]
    );
    assert_eq!(
        check("fn f(x) { return x; } print(x);"),
        vec!["1:29 undefined variable 'x'"]
    );
    assert_eq!(
        check("if true { let y = 1; } print(y);"),
        vec!["1:30 undefined variable 'y'"]
    );
    // Closures see enclosing names; a loop outside a fn doesn't license break inside it.
    assert_eq!(
        check("let n = 1; fn g() { return n; }"),
        Vec::<String>::new()
    );
    assert_eq!(
        check("while true { fn h() { break; } }"),
        vec!["1:23 'break' outside of a loop"]
    );
}
