use limits::ast::{Expr, Stmt, UnOp};
use limits::error::{Kind, Span};
use limits::semantic::{self, SemanticIndex, Symbol};
use limits::{lexer, parser, resolve};

fn named<'a>(index: &'a SemanticIndex, name: &str) -> Vec<&'a Symbol> {
    index.symbols.iter().filter(|s| s.name == name).collect()
}

#[test]
fn resolver_rejects_malformed_public_asts_before_name_checking() {
    let span = Span { line: 7, col: 11 };
    let program = vec![
        Stmt::Expr(Expr::Var("missing".into(), span), span),
        Stmt::Assign(Expr::Int(1), Expr::Var("other".into(), span), span),
    ];
    let errors = resolve::check(&program);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].kind, Kind::Parse);
    assert_eq!(errors[0].msg, "invalid assignment target");
    assert_eq!(errors[0].span, span);
}

#[test]
fn resolver_rejects_deep_public_expressions_and_function_bodies() {
    let span = Span { line: 9, col: 4 };
    let mut expression = Expr::Var("missing".into(), span);
    for _ in 0..20_000 {
        expression = Expr::Unary(UnOp::Not, Box::new(expression), span);
    }
    let program = vec![Stmt::Expr(expression, span)];
    assert_nesting_error(program, span);

    let mut program = vec![Stmt::Break(span)];
    for _ in 0..20_000 {
        program = vec![Stmt::Fn("f".into(), Vec::new(), program, span)];
    }
    assert_nesting_error(program, span);
}

fn assert_nesting_error(program: Vec<Stmt>, span: Span) {
    struct DeepProgram(Vec<Stmt>);
    impl Drop for DeepProgram {
        fn drop(&mut self) {
            drop_without_recursion(std::mem::take(&mut self.0));
        }
    }
    // Cleanup must remain iterative even if a regression panics an assertion.
    let program = DeepProgram(program);
    let errors = resolve::check(&program.0);
    // The shared AST guard fails closed rather than reporting a partial set
    // of ordinary name/control-flow diagnostics from an unsupported tree.
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].kind, Kind::Parse);
    assert_eq!(errors[0].msg, "AST nesting limit exceeded (256)");
    assert_eq!(errors[0].span, span);
}

// The public AST's owner remains responsible for dropping caller-built trees.
// Consume the deliberately extreme test fixtures without recursive Box drops.
fn drop_without_recursion(program: Vec<Stmt>) {
    enum Node {
        Stmt(Stmt),
        Expr(Expr),
    }
    let mut work: Vec<_> = program.into_iter().map(Node::Stmt).collect();
    while let Some(node) = work.pop() {
        match node {
            Node::Stmt(Stmt::Fn(_, _, body, _)) => {
                work.extend(body.into_iter().map(Node::Stmt));
            }
            Node::Stmt(Stmt::Expr(expression, _)) => work.push(Node::Expr(expression)),
            Node::Expr(Expr::Unary(_, expression, _)) => work.push(Node::Expr(*expression)),
            Node::Stmt(Stmt::Break(_)) | Node::Expr(Expr::Var(..)) => {}
            _ => panic!("unexpected deep-test fixture"),
        }
    }
}

#[test]
fn resolver_preserves_all_parse_valid_diagnostics_and_function_loop_boundaries() {
    let source = "while true {\n\
        fn f(a,a) { break; continue; return unknown; }\n\
        continue;\n\
    }\n\
    missing=other;\n\
    for item in item { print(item); }\n\
    print(item);";
    let program = parser::parse(lexer::lex(source).unwrap()).unwrap();
    let errors = resolve::check(&program);
    assert!(errors.iter().all(|error| error.kind == Kind::Check));
    assert_eq!(
        errors
            .iter()
            .map(|error| error.msg.as_str())
            .collect::<Vec<_>>(),
        [
            "duplicate parameter 'a' in fn f",
            "'break' outside of a loop",
            "'continue' outside of a loop",
            "undefined variable 'unknown'",
            "assignment to undeclared variable 'missing' (use 'let')",
            "undefined variable 'other'",
            "undefined variable 'item'",
            "undefined variable 'item'",
        ]
    );
    assert_eq!(semantic::analyze(source).unwrap().diagnostics, errors);
}

#[test]
fn nested_captures_include_late_user_fallbacks_and_exclude_builtins() {
    let source = "let len=0; fn outer(len) { if true {\n\
        fn inner() { return len; }\n\
        let len=1;\n\
    } }";
    let index = semantic::analyze(source).unwrap();
    assert!(index.diagnostics.is_empty());
    let lens = named(&index, "len");
    assert_eq!(lens.len(), 4);
    let reference = index.references.iter().find(|r| r.name == "len").unwrap();
    assert_eq!(
        reference.candidates,
        [lens[3].id, lens[2].id, lens[1].id, lens[0].id]
    );
    assert_eq!(reference.binding, Some(lens[3].id));
    assert_eq!(
        named(&index, "inner")[0].captures,
        [lens[1].id, lens[2].id, lens[3].id]
    );
    assert!(named(&index, "outer")[0].captures.is_empty());
    for symbol in lens {
        assert_eq!(index.references_to(symbol.id), vec![reference]);
    }
}

#[test]
fn redeclared_function_bindings_keep_captures_from_each_body() {
    let source = "let x=0; let f=nil;\n\
        fn f(p) { let x=p; fn inner(q) { return x+p+q+late; } return inner; }\n\
        fn f(r) { return x+r; }\n\
        let late=1;";
    let index = semantic::analyze(source).unwrap();
    assert!(index.diagnostics.is_empty());
    let f = named(&index, "f");
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].kind, "binding");
    assert_eq!(f[0].declarations.len(), 3);
    assert_eq!(f[0].parameters, ["r"]);
    let xs = named(&index, "x");
    assert_eq!(f[0].captures, [xs[0].id]);
    let mut inner_captures = vec![
        xs[0].id,
        xs[1].id,
        named(&index, "p")[0].id,
        named(&index, "late")[0].id,
    ];
    inner_captures.sort_unstable();
    assert_eq!(named(&index, "inner")[0].captures, inner_captures);

    // Every scope, symbol and reference ID addresses the corresponding table.
    for (id, scope) in index.scopes.iter().enumerate() {
        assert_eq!(scope.id, id);
        assert!(scope.parent.is_none_or(|parent| parent < id));
        assert!(scope
            .owner_function
            .is_none_or(|owner| owner < index.symbols.len()));
    }
    for (id, symbol) in index.symbols.iter().enumerate() {
        assert_eq!(symbol.id, id);
        assert!(symbol.scope < index.scopes.len());
    }
    for reference in &index.references {
        assert!(reference.scope < index.scopes.len());
        assert!(reference
            .candidates
            .iter()
            .all(|&id| index.symbols[id].name == reference.name));
    }
}

#[test]
fn only_direct_variable_callees_and_assignment_targets_change_reference_roles() {
    let source = "let f=nil;let g=nil;let a=[f];let i=0;\n\
        f(); (f)(); ((f))(); f()();\n\
        a[i](); (f or g)(); (-f)(); (not f)();\n\
        f(g); ((f))=g; a[f()]=g;";
    let index = semantic::analyze(source).unwrap();
    assert!(index.diagnostics.is_empty());
    let roles = |line| {
        index
            .references
            .iter()
            .filter(|r| r.range.start.line == line)
            .map(|r| (r.name.as_str(), r.role.as_str()))
            .collect::<Vec<_>>()
    };
    assert_eq!(roles(2), [("f", "call"); 4]);
    assert_eq!(
        roles(3),
        [
            ("a", "read"),
            ("i", "read"),
            ("f", "read"),
            ("g", "read"),
            ("f", "read"),
            ("f", "read")
        ]
    );
    assert_eq!(
        roles(4),
        [
            ("f", "call"),
            ("g", "read"),
            ("f", "write"),
            ("g", "read"),
            ("a", "read"),
            ("f", "call"),
            ("g", "read")
        ]
    );
}
