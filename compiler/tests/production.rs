use limits::error::{Kind, LimError};
use limits::interp::{Interpreter, Limits};
use limits::{codegen, lexer, parser, vm::Vm};
use std::io::{self, Write};

fn execute(src: &str, vm: bool, limits: Limits) -> (Vec<u8>, Result<(), LimError>) {
    let mut out = Vec::new();
    let result = lexer::lex(src).and_then(parser::parse).and_then(|p| {
        if vm {
            Vm::with_limits(&mut out, limits).run(codegen::compile(&p)?)
        } else {
            Interpreter::with_limits(&mut out, limits).run(&p)
        }
    });
    (out, result)
}
fn parity(src: &str) -> (Vec<u8>, Result<(), LimError>) {
    let limits = Limits {
        max_steps: 100_000,
        max_depth: 64,
    };
    let tree = execute(src, false, limits);
    assert_eq!(tree, execute(src, true, limits), "{src}");
    tree
}
#[test]
fn cycles_can_be_printed_and_compared() {
    let (out, r) = parity(
        r#"
        let a=[];push(a,a);let b=[];push(b,b);
        print(a,a==b);
        push(b,1);print(a==b);
        let m={};m["self"]=m;print(m,m==m);
        let x=[1];print([x,x]);
    "#,
    );
    r.unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "[[...]] true\nfalse\n{\"self\": {...}} true\n[[1], [1]]\n"
    );
}
#[test]
fn scalar_and_nan_equality_stays_compatible() {
    let (out, r) = parity(r#"let n=float("NaN");let xs=[n];print(xs==xs,n==n,1==1.0,[1]==[1.0]);"#);
    r.unwrap();
    assert_eq!(out, b"false false true true\n");
}
#[test]
fn exponentially_shared_output_is_bounded() {
    let (out, r) = parity("let x=[0];for i in 0..40 {x=[x,x];}print(x);");
    assert!(out.is_empty());
    let e = r.unwrap_err();
    assert_eq!(e.kind, Kind::Runtime);
    assert_eq!(e.msg, "value rendering limit exceeded");
    assert!(e.span.col > 1);
}
#[test]
fn shared_dag_comparison_does_not_expand_exponentially() {
    let (out, r) = parity("let x=[0];let y=[0];for i in 0..40 {x=[x,x];y=[y,y];}print(x==y);");
    r.unwrap();
    assert_eq!(out, b"true\n");
}
#[test]
fn parser_rejects_deep_or_flat_adversarial_expressions() {
    let cases = [
        format!("print({}1{});", "(".repeat(10_000), ")".repeat(10_000)),
        format!("print({}1);", "-".repeat(10_000)),
        format!("print({}true);", "not ".repeat(10_000)),
        format!("print({});", vec!["1"; 10_000].join("+")),
        format!("print({});", vec!["1"; 10_000].join("*")),
        format!("print({});", vec!["true"; 10_000].join(" and ")),
        format!("print({});", vec!["false"; 10_000].join(" or ")),
        format!("let a=[];print(a{});", "[0]".repeat(10_000)),
        format!("let f=print;f{};", "()".repeat(10_000)),
        format!("{}{}", "if true {".repeat(10_000), "}".repeat(10_000)),
        format!("if false {{}} {} {{}}", "else if false {} ".repeat(10_000)),
    ];
    for src in cases {
        let err = parser::parse(lexer::lex(&src).unwrap()).unwrap_err();
        assert_eq!(err.kind, Kind::Parse);
        assert!(err.msg.contains("nesting limit"), "{err}");
        assert!(err.span.line > 0 && err.span.col > 0);
    }
}
#[test]
fn gc_preserves_escaped_closures_and_iterator_snapshots() {
    let (out, r) = parity(
        r#"
        let fs=[];
        for i in 0..5000 {fn f(){return i;}if i%1000==0 {push(fs,f);}}
        let xs=[[9]];
        for x in xs {
            xs[0]=nil;
            for i in 0..5000 {let unused=[];}
            print(x);
        }
        for f in fs {print(f());}
    "#,
    );
    r.unwrap();
    assert_eq!(out, b"[9]\n0\n1000\n2000\n3000\n4000\n");
}
#[test]
fn vm_deep_container_lifetime_does_not_use_native_stack() {
    let (out, r) = execute(
        "let x=nil;for i in 0..50000 {x=[x];}print(len(x));",
        true,
        Limits {
            max_steps: 200_000,
            max_depth: 64,
        },
    );
    r.unwrap();
    assert_eq!(out, b"1\n");
}
#[test]
fn vm_invocations_are_isolated_after_success_and_errors() {
    let program = |s| codegen::compile(&parser::parse(lexer::lex(s).unwrap()).unwrap()).unwrap();
    let good = program("print(1);");
    let bad = program("while true {}");
    let mut out = Vec::new();
    {
        let mut vm = Vm::with_limits(
            &mut out,
            Limits {
                max_steps: 2,
                max_depth: 1,
            },
        );
        for _ in 0..100 {
            assert!(vm.run(bad.clone()).unwrap_err().msg.contains("step limit"));
            vm.run(good.clone()).unwrap();
        }
    }
    assert_eq!(out, b"1\n".repeat(100));
}
struct Broken;
impl Write for Broken {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("sink failed"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn writer_errors_preserve_runtime_error_and_span() {
    let src = "\nprint(1);";
    let prog = parser::parse(lexer::lex(src).unwrap()).unwrap();
    let tree = Interpreter::new(&mut Broken).run(&prog).unwrap_err();
    let vm = Vm::new(&mut Broken)
        .run(codegen::compile(&prog).unwrap())
        .unwrap_err();
    assert_eq!(tree, vm);
    assert_eq!(vm.kind, Kind::Runtime);
    assert_eq!(vm.msg, "sink failed");
    assert_eq!((vm.span.line, vm.span.col), (2, 6));
}
#[test]
fn deterministic_generated_programs_cover_optimizer_paths() {
    let mut seed = 0xA94E_6211_u64;
    for _ in 0..300 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let n = (seed >> 32) % 19;
        let src = format!(
            r#"
            let x={n};
            fn f(n) {{if n<2 {{return n;}}n=n-1;return n*2;}}
            fn make(n) {{fn get(){{n=n+1;return n;}}return get;}}
            let fs=[];for i in 0..8 {{if i=={n}%8 {{continue;}}push(fs,make(i));}}
            let s=0;for g in fs {{s=s+g()+g();}}
            print(f(x),s,x+1,x<3,x==4,{{"x":[x]}});
        "#
        );
        let (_, r) = parity(&src);
        r.unwrap();
    }
}

#[test]
fn both_engines_stop_at_a_hundred_thousand_calls() {
    let src = "fn f(){return f();}f();";
    let limits = Limits {
        max_steps: 1_000_000,
        max_depth: 100_000,
    };
    let tree = execute(src, false, limits);
    let vm = execute(src, true, limits);
    assert_eq!(tree, vm);
    assert_eq!(vm.1.unwrap_err().msg, "call depth limit exceeded (100000)");
}
#[test]
fn deep_container_cleanup_is_safe_in_both_engines() {
    let src = "let x=nil;for i in 0..30000 {x=[x];}print(len(x));";
    let (out, r) = parity(src);
    r.unwrap();
    assert_eq!(out, b"1\n");
}
#[test]
fn invalid_embedding_inputs_are_errors() {
    use limits::{
        ast::{Expr, Stmt},
        error::Span,
    };
    assert_eq!(parser::parse(Vec::new()).unwrap_err().kind, Kind::Parse);
    let ast = vec![Stmt::Assign(
        Expr::Int(1),
        Expr::Int(2),
        Span { line: 3, col: 4 },
    )];
    let err = match codegen::compile(&ast) {
        Err(e) => e,
        Ok(_) => panic!("invalid AST compiled"),
    };
    assert_eq!(err.kind, Kind::Parse);
    assert_eq!(err.msg, "invalid assignment target");
    assert_eq!((err.span.line, err.span.col), (3, 4));
}
#[test]
fn public_api_selects_both_engines_and_preserves_partial_output() {
    use limits::{run_with_engine, Engine};
    let mut results = Vec::new();
    for engine in [Engine::Tree, Engine::Vm] {
        let mut out = Vec::new();
        let result = run_with_engine(
            "print(1);\nprint(1/0);",
            &mut out,
            Limits::default(),
            engine,
        );
        assert_eq!(out, b"1\n");
        results.push((out, result));
    }
    assert_eq!(results[0], results[1]);
}

#[test]
fn source_size_is_bounded_before_token_allocation() {
    let src = " ".repeat(lexer::MAX_SOURCE_BYTES + 1);
    let e = lexer::lex(&src).unwrap_err();
    assert_eq!(e.kind, Kind::Lex);
    assert_eq!(e.msg, "source size limit exceeded (16777216 bytes)");
    assert_eq!((e.span.line, e.span.col), (1, 1));
}
#[test]
fn runaway_cli_has_an_external_watchdog() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/runaway.lim");
    for engine in ["tree", "vm"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lim"))
            .arg("run")
            .arg(&file)
            .args(["--engine", engine, "--max-steps", "1000"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{engine} did not stop before watchdog deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .contains("runtime error at 4:1: step limit exceeded (1000 steps)"));
    }
}

#[test]
fn full_width_range_with_full_width_budget_is_an_error_not_a_capacity_panic() {
    let source = "print((-9223372036854775807-1)..9223372036854775807);";
    let limits = Limits {
        max_steps: u64::MAX,
        max_depth: 64,
    };
    let tree = execute(source, false, limits);
    let vm = execute(source, true, limits);
    assert_eq!(tree, vm);
    let e = vm.1.unwrap_err();
    assert_eq!(e.kind, Kind::Runtime);
    assert_eq!(e.msg, "range allocation exceeds host capacity");
}
