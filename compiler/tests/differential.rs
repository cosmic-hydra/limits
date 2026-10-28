use limits::error::LimError;
use limits::interp::{Interpreter, Limits};
use limits::{codegen, lexer, parser, vm::Vm};

fn execute(src: &str, limits: Limits, vm: bool) -> (Vec<u8>, Result<(), LimError>) {
    let mut out = Vec::new();
    let result = lexer::lex(src).and_then(parser::parse).and_then(|p| {
        if vm {
            Vm::with_limits(&mut out, limits).run(codegen::compile(&p))
        } else {
            Interpreter::with_limits(&mut out, limits).run(&p)
        }
    });
    (out, result)
}
fn compare(src: &str, limits: Limits) {
    assert_eq!(
        execute(src, limits, false),
        execute(src, limits, true),
        "source: {src}\nlimits: {limits:?}"
    );
}
// Original lang.rs programs, closure/control-flow/aliasing stress cases, and errors.
const PROGRAMS: &[&str] = &[
    r###"
        fn even(n) { if n == 0 { return true; } return odd(n - 1); }
        fn odd(n) { if n == 0 { return false; } return even(n - 1); }
        let total = 0;
        for i in 0..3 { total = total + i; if i > 1 { break; } }
        while false { continue; }
        print(even(4), len([total]));
    "###,
    r###"print(totl);
count = 1;
break;
fn f(a, a) { continue; }
let m = {"k": undefined_value};"###,
    r###"for i in [1] { } print(i);"###,
    r###"fn f(x) { return x; } print(x);"###,
    r###"if true { let y = 1; } print(y);"###,
    r###"let n = 1; fn g() { return n; }"###,
    r###"while true { fn h() { break; } }"###,
    r###"fn f(n){ if true {n=n+1;} print(n); if true {n=n+1;let n=9;print(n);}return n;}print(f(1));"###,
    r###"fn f(n){for i in 0..3 {n=n+1;}return n;}print(f(1));"###,
    r###"fn f(n){let n=n+1;return n;}print(f(2));"###,
    r###"let f=1;fn g(f){return f-1;}print(g(3));"###,
    r###"fn f(n){n=n+1;return n;}print(f(2));"###,
    r###"fn f(n){if n<2{return 1;}return f(n-1)+1;}print(f(500));"###,
    r###"print(9007199254740992 < 9007199254740993,9007199254740992 == 9007199254740993);"###,
    r###"print(1 + 2 * 3, (1 + 2) * 3, 7 / 2, 7 % 3, -2 * 3);"###,
    r###"print(1 + 0.5, 3.0);"###,
    r###"print("a" + "b", len("héllo"), "abc"[-1]);"###,
    r###"let l = [1, 2]; push(l, 3); l[0] = 9; print(l, len(l), pop(l));"###,
    r###"let m = {"a": 1}; m["b"] = "x"; print(m, m["zz"], keys(m));"###,
    r###"
        let s = 0;
        for i in 0..10 {
            if i == 2 { continue; }
            if i == 5 { break; }
            s = s + i;
        }
        let w = 0;
        while w < 3 { w = w + 1; }
        if false { print(0); } else if s == 8 { print(s, w); } else { print(-1); }
    "###,
    r###"
        fn make_adder(n) { fn add(x) { return x + n; } return add; }
        let add2 = make_adder(2);
        fn fact(n) { if n <= 1 { return 1; } return n * fact(n - 1); }
        print(add2(5), fact(10));
    "###,
    r###"print(false and undefined_thing, true or undefined_thing, not nil);"###,
    r###"let x = 1; if true { let x = 2; } print(x);"###,
    r###"let x = 1; if true { x = 2; } print(x);"###,
    r###"let x = 1;
print(y);"###,
    r###"x = 1;"###,
    r###"print(1 / 0);"###,
    r###"print([1][5]);"###,
    r###"print(9223372036854775807 + 1);"###,
    r###"print(1 + "a");"###,
    r###"assert(false, "boom");"###,
    r###"break;"###,
    r###"let m = {1: 2};"###,
    r###"let = 3;"###,
    r###"print(1)"###,
    r###"print(1 < 2 < 3);"###,
    r###"1 + 2 = 3;"###,
    r###"let s = "open;"###,
    r###"let a = 1 @ 2;"###,
    r###"while true { }"###,
    r###"fn f(n) { return f(n + 1); } f(0);"###,
    r###"let fs=[]; for i in 0..4 { fn f(){return i;} push(fs,f); } print(fs[0](),fs[1](),fs[3]());"###,
    r###"let fs=[]; let i=0; while i<4 {let n=i; fn f(){return n;} push(fs,f);i=i+1;} print(fs[0](),fs[3]());"###,
    r###"let fs=[]; for i in 0..3 { fn f(){i=i+1;return i;} push(fs,f); } print(fs[0](),fs[0](),fs[1]());"###,
    r###"fn make(){let x=0;fn inc(){x=x+1;return x;}fn get(){return x;}return [inc,get];}let a=make();print(a[0](),a[1](),a[0]());"###,
    r###"fn make(){fn get(){return x;}let x=7;return get;}print(make()());"###,
    r###"let x=1;if true {fn f(){return x;} print(f());let x=2;print(f());}"###,
    r###"let x=1; if true {x=3;let x=x+1;print(x);} print(x);"###,
    r###"let s=0;for i in 0..4 {for j in 0..5 {if j==1 {continue;}if j==3 {break;}s=s+i+j;}}print(s);"###,
    r###"let i=0;while i<4 {i=i+1;for j in 0..4 {if j==1 {continue;}if i==3 {break;}print(i,j);}}"###,
    r###"for i in 0..3 {let j=0;while true {j=j+1;if j==2 {break;}print(i,j);}continue;}"###,
    r###"fn f(n){if n==0 {return 9;}return f(n-1);}print(f(15));"###,
    r###"fn f(n){if n==0 {return 9;}return f(n-1);}print(f(16));"###,
    r###"fn even(n){if n==0{return true;}return odd(n-1);}fn odd(n){if n==0{return false;}return even(n-1);}print(even(15));"###,
    r###"let a=[1];let b=a;b[0]=2;push(a,3);print(a,b,a==b);"###,
    r###"let a={"x":[1]};let b=a;push(b["x"],2);b["y"]=3;print(a,b);"###,
    r###"let a=[1,2,3];for x in a {pop(a);print(x);}print(a);"###,
    r###"let m={"b":2,"a":1};for k in m {m["c"]=3;print(k);}print(m);"###,
    r###"fn f(n){for i in 0..5 {if i==n{return i;}}return nil;}print(f(2),f(9));"###,
    r###"fn f(){break;}for i in 0..2 {f();}"###,
    r###"fn f(){continue;}while true {f();}"###,
    r###"let x=10;fn f(){print(x);let x=20;print(x);}f();"###,
    r###"let a=[];for i in 0..3 {for j in 0..2 {fn f(){return i*10+j;}push(a,f);}}print(a[0](),a[3](),a[5]());"###,
    r###"fn f(a,a){return a;} print(f(1,2));"###,
    r###"let x=1;let x=x+1;fn x(){return 3;} print(x());"###,
    r###"print(false and assert(false),true or assert(false),0 and 4,nil or 3);"###,
    r###"let a=[1];fn mutate(){push(a,2);return 9;}a[len(a)-1]=mutate();print(a);"###,
    r###"let m={1:assert(false,"should not run")};"###,
    r###"print((-9223372036854775807-1)..9223372036854775807);"###,
    r###"for i in [1,2,3] {} print(1);"###,
    r###"fn make(n){if n==0 {fn f(){return 1;}return f;}let f=make(n-1);fn g(){return f()+1;}return g;}print(make(8)());"###,
    r###"continue;"###,
    r###"return 1; print(undefined);"###,
    r###"print(1());"###,
    r###"fn f(a){}f();"###,
    r###"print(-true);"###,
    r###"print(-(-9223372036854775807-1));"###,
    r###"print(1.."x");"###,
    r###"for i in 2 {}"###,
    r###"print(2[0]);"###,
    r###"let a=[1];a[3]=2;"###,
    r###"let a="x";a[0]=2;"###,
    r###"print("x"[2]);"###,
    r###"print({}[0]);"###,
    r###"print(1/0.0);"###,
    r###"print(1%0);"###,
    r###"print("a"<"b");"###,
    r###"print(len(1));"###,
    r###"print(pop([]));"###,
    r###"print(pop(1));"###,
    r###"print(push(1,2));"###,
    r###"print(keys([]));"###,
    r###"print(int("x"));"###,
    r###"print(float("x"));"###,
    r###"print(int([]));"###,
    r###"print(float([]));"###,
    r###"assert();"###,
    r###"assert(false);"###,
    r###"print(len());"###,
];

#[test]
fn all_language_programs_and_stress_cases() {
    for src in PROGRAMS {
        compare(
            src,
            Limits {
                max_steps: 10_000,
                ..Limits::default()
            },
        );
        for steps in [0, 1, 2, 3, 5, 10, 20, 50, 100] {
            compare(
                src,
                Limits {
                    max_steps: steps,
                    max_depth: 16,
                },
            );
        }
        for depth in [0, 1, 8, 15, 16, 17] {
            compare(
                src,
                Limits {
                    max_steps: 2000,
                    max_depth: depth,
                },
            );
        }
    }
}
#[test]
fn every_example() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "lim") {
            let src = std::fs::read_to_string(&path).unwrap();
            compare(&src, Limits::default());
            compare(
                &src,
                Limits {
                    max_steps: 100,
                    max_depth: 16,
                },
            );
        }
    }
}
#[test]
fn vm_uses_explicit_frames_at_large_depth() {
    let limits = Limits {
        max_steps: 1_000_000,
        max_depth: 100_000,
    };
    let (_, result) = execute("fn f(){return f();}f();", limits, true);
    assert!(result
        .unwrap_err()
        .msg
        .contains("call depth limit exceeded (100000)"));
}
#[test]
fn for_iterations_are_charged() {
    let src = "for x in [1, 2, 3] {}";
    let limits = Limits {
        max_steps: 2,
        max_depth: 16,
    };
    compare(src, limits);
    let e = execute(src, limits, false).1.unwrap_err();
    assert_eq!(e.msg, "step limit exceeded (2 steps)");
    assert_eq!((e.span.line, e.span.col), (1, 1));
}
#[test]
fn extreme_range_is_a_runtime_error() {
    let src = "print((-9223372036854775807-1)..9223372036854775807);";
    compare(
        src,
        Limits {
            max_depth: 50,
            ..Limits::default()
        },
    );
    assert_eq!(
        execute(src, Limits::default(), false).1.unwrap_err().msg,
        "range of 18446744073709551615 elements exceeds step limit"
    );
}

#[test]
fn default_tree_depth_reports_an_error() {
    let src = "fn f(n) { return f(n + 1); } f(0);";
    compare(src, Limits::default());
    assert!(execute(src, Limits::default(), false)
        .1
        .unwrap_err()
        .msg
        .contains("call depth limit exceeded (256)"));
}

#[test]
fn cli_engines_and_limits() {
    use std::process::Command;
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/hello.lim");
    let invoke = |flags: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_lim"))
            .arg("run")
            .arg(&file)
            .args(flags)
            .output()
            .unwrap()
    };
    let default = invoke(&[]);
    let tree = invoke(&["--engine", "tree"]);
    let vm = invoke(&["--engine", "vm"]);
    assert!(default.status.success());
    assert_eq!(default.stdout, b"hello, limits\n");
    for output in [tree, vm] {
        assert_eq!(output.status.code(), default.status.code());
        assert_eq!(output.stdout, default.stdout);
        assert_eq!(output.stderr, default.stderr);
    }
    let tree = invoke(&["--engine", "tree", "--max-steps", "0"]);
    let vm = invoke(&["--max-steps", "0", "--engine", "vm"]);
    assert_eq!(tree.status.code(), Some(1));
    assert_eq!(tree.stdout, vm.stdout);
    assert_eq!(tree.stderr, vm.stderr);
    for flags in [&["--engine"][..], &["--engine", "bad"][..]] {
        assert_eq!(invoke(flags).status.code(), Some(2));
    }
}
