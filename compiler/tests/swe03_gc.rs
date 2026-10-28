//! Public-API regressions for values whose only roots are suspended work.
use limits::error::Kind;
use limits::interp::{Interpreter, Limits};
use limits::{lexer, parser, run_with_engine, Engine};

fn assert_both(source: &str, expected: &[u8]) {
    for engine in [Engine::Tree, Engine::Vm] {
        let mut output = Vec::new();
        run_with_engine(source, &mut output, Limits::default(), engine)
            .unwrap_or_else(|error| panic!("{engine:?}: {error}"));
        assert_eq!(output, expected, "{engine:?}");
    }
}

#[test]
fn partially_evaluated_maps_and_arguments_survive_collection() {
    assert_both(
        r#"
        fn churn() {
            for i in 0..6000 { let garbage=[i]; }
            return 7;
        }
        fn key() { churn(); return "last"; }
        fn first(a,b) { return a[0]["value"][0]; }
        let m={"first":[{"deep":[9]}], "middle":churn(), key():[10]};
        print(m["first"][0]["deep"][0],m["middle"],m["last"][0]);
        print(first([{"value":[13]}],churn()));
        "#,
        b"9 7 10\n13\n",
    );
}

#[test]
fn unconsumed_iterator_values_survive_after_source_is_cleared() {
    assert_both(
        r#"
        let source=[[1],[2],[3]];
        for item in source {
            source[0]=nil;source[1]=nil;source[2]=nil;
            for i in 0..6000 { let garbage={"n":i}; }
            print(item[0]);
        }
        "#,
        b"1\n2\n3\n",
    );
}

#[test]
fn mixed_cycles_reachable_only_through_a_closure_keep_shared_writes() {
    assert_both(
        r#"
        fn capture(x) { fn get() { return x; } return get; }
        let list=[];
        let map={"back":list};
        push(list,map);
        let holder=[{"get":capture(list)}];
        list=nil;map=nil;
        for i in 0..6000 { let garbage=[{"n":i}]; }
        let recovered=holder[0]["get"]();
        recovered[0]["answer"]=42;
        print(holder[0]["get"]()[0]["back"][0]["answer"]);
        recovered=nil;holder=nil;
        for i in 0..6000 { let garbage=[i]; }
        print("done");
        "#,
        b"42\ndone\n",
    );
}

#[test]
fn retained_tree_globals_survive_a_later_run_and_error_cleanup() {
    let parse = |source| parser::parse(lexer::lex(source).unwrap()).unwrap();
    let setup = parse(
        r#"
        let saved=nil;
        fn build() {
            let root=[{"value":[11]}];
            fn get() { return root; }
            saved=get;
        }
        build();
        "#,
    );
    let fail = parse("for i in 0..6000 { let garbage=[{\"n\":i}]; } assert(false);");
    let inspect = parse("print(saved()[0][\"value\"][0]);saved=nil;");
    let mut output = Vec::new();
    {
        let mut interpreter = Interpreter::new(&mut output);
        interpreter.run(&setup).unwrap();
        assert_eq!(interpreter.run(&fail).unwrap_err().msg, "assertion failed");
        interpreter.run(&inspect).unwrap();
    }
    assert_eq!(output, b"11\n");
}

#[test]
fn deep_mixed_graphs_release_on_success_and_runtime_error() {
    // An abort from a recursive native destructor must fail this regression
    // without taking down the rest of the integration-test process.
    const CHILD: &str = "LIMITS_SWE03_DEEP_GRAPH_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "deep_mixed_graphs_release_on_success_and_runtime_error",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    for engine in [Engine::Tree, Engine::Vm] {
        for fail in [false, true] {
            let end = if fail { "assert(false);" } else { "print(1);" };
            let source = format!(
                r#"
                fn capture(parent) {{ fn next() {{ return parent; }} return next; }}
                fn build() {{
                    let graph=nil;
                    let chain=nil;
                    for i in 0..30000 {{
                        graph=[{{"previous":graph}}];
                        chain=capture(chain);
                    }}
                    {end}
                }}
                build();
                for i in 0..6000 {{ let garbage=[i]; }}
                "#
            );
            let mut output = Vec::new();
            let result = run_with_engine(&source, &mut output, Limits::default(), engine);
            if fail {
                let error = result.unwrap_err();
                assert_eq!(error.kind, Kind::Runtime, "{engine:?}");
                assert_eq!(error.msg, "assertion failed", "{engine:?}");
                assert!(output.is_empty(), "{engine:?}");
            } else {
                result.unwrap_or_else(|error| panic!("{engine:?}: {error}"));
                assert_eq!(output, b"1\n", "{engine:?}");
            }
        }
    }
}
