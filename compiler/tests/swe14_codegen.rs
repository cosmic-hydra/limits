use limits::ast::{BinOp, Expr, Program, Stmt};
use limits::error::{LimError, Span};
use limits::interp::{Interpreter, Limits};
use limits::{codegen, lexer, parser, vm::Vm};

fn execute(source: &str, limits: Limits, vm: bool) -> (Vec<u8>, Result<(), LimError>) {
    let ast = parser::parse(lexer::lex(source).unwrap()).unwrap();
    execute_ast(&ast, limits, vm)
}

fn execute_ast(ast: &Program, limits: Limits, vm: bool) -> (Vec<u8>, Result<(), LimError>) {
    let mut output = Vec::new();
    let result = if vm {
        Vm::with_limits(&mut output, limits).run(codegen::compile(ast).unwrap())
    } else {
        Interpreter::with_limits(&mut output, limits).run(ast)
    };
    (output, result)
}

#[test]
fn short_circuit_conditions_keep_their_final_branch() {
    let source = r#"
        let x=1;
        if false and missing+1 {print("wrong");} else {print("false and");}
        if true or missing+1 {print("true or");}
        if (true or missing+1) and (false or x+1) {print("nested");}
        while false and missing+1 {print("wrong loop");}
        let i=0;
        while true or missing+1 {i=i+1;if i==3 {break;}}
        print(i);
    "#;
    let limits = Limits {
        max_steps: 100,
        max_depth: 16,
    };
    let expected = execute(source, limits, false);
    assert_eq!(expected.0, b"false and\ntrue or\nnested\n3\n");
    expected.1.as_ref().unwrap();
    assert_eq!(execute(source, limits, true), expected);
}

#[test]
fn short_circuit_conditions_preserve_calls_and_frame_results() {
    let source = r#"
        let calls=0;
        fn truth(value){calls=calls+1;return value;}
        fn identity(value){return value;}
        fn choose(n){
            if truth(false) and n+1 {return 99;}
            if truth(true) or missing+1 {n=n+1;}
            while truth(n<4) and n+1 {n=n+1;}
            while true or missing+1 {if n==6 {break;}n=n+1;}
            return identity(n+1);
        }
        print(10+choose(1)+20,calls);
    "#;
    let expected = execute(source, Limits::default(), false);
    assert_eq!(expected.0, b"37 5\n");
    expected.1.as_ref().unwrap();
    assert_eq!(execute(source, Limits::default(), true), expected);
}

#[test]
fn fused_expressions_preserve_error_order_and_public_ast_spans() {
    let statement_span = Span { line: 10, col: 1 };
    let callee_span = Span { line: 11, col: 2 };
    let argument_span = Span { line: 12, col: 3 };
    let expression_span = Span { line: 13, col: 4 };
    let call_span = Span { line: 14, col: 5 };
    let binary = Expr::Binary(
        BinOp::Add,
        Box::new(Expr::Var("argument".into(), argument_span)),
        Box::new(Expr::Int(1)),
        expression_span,
    );
    let call = Expr::Call(
        Box::new(Expr::Var("callee".into(), callee_span)),
        vec![binary.clone()],
        call_span,
    );
    let define_callee = Stmt::Let("callee".into(), Expr::Int(0), statement_span);
    let define_argument = |value| Stmt::Let("argument".into(), value, statement_span);
    let cases = [
        (vec![], callee_span),
        (vec![define_callee.clone()], argument_span),
        (
            vec![define_callee.clone(), define_argument(Expr::Bool(true))],
            expression_span,
        ),
        (
            vec![define_callee, define_argument(Expr::Int(2))],
            call_span,
        ),
    ];
    for (mut ast, expected_span) in cases {
        ast.push(Stmt::Expr(call.clone(), statement_span));
        let expected = execute_ast(&ast, Limits::default(), false);
        assert_eq!(expected.1.as_ref().unwrap_err().span, expected_span);
        assert_eq!(execute_ast(&ast, Limits::default(), true), expected);
        // Argument errors occur before the call's step charge, including at
        // budgets that leave exactly enough steps to enter the expression.
        for max_steps in 0..=4 {
            let limits = Limits {
                max_steps,
                max_depth: 16,
            };
            assert_eq!(
                execute_ast(&ast, limits, true),
                execute_ast(&ast, limits, false)
            );
        }
    }
    for statement in [
        Stmt::Expr(binary.clone(), statement_span),
        Stmt::If(binary.clone(), vec![], None, statement_span),
        Stmt::While(binary, vec![], statement_span),
    ] {
        for (definitions, expected_span) in [
            (vec![], argument_span),
            (vec![define_argument(Expr::Bool(true))], expression_span),
        ] {
            let mut ast = definitions;
            ast.push(statement.clone());
            let expected = execute_ast(&ast, Limits::default(), false);
            assert_eq!(expected.1.as_ref().unwrap_err().span, expected_span);
            assert_eq!(execute_ast(&ast, Limits::default(), true), expected);
        }
    }
}
