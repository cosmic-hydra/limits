use limits::error::{Kind, LimError, Span};
use limits::interp::{Limits, Value};
use limits::{run_with_engine, stdlib, Engine};
use std::{cell::RefCell, rc::Rc};

fn run(src: &str) -> (Vec<u8>, Result<(), LimError>) {
    let mut results = Vec::new();
    for engine in [Engine::Tree, Engine::Vm] {
        let mut out = Vec::new();
        let result = run_with_engine(
            src,
            &mut out,
            Limits {
                max_steps: 10000,
                max_depth: 64,
            },
            engine,
        );
        results.push((out, result));
    }
    assert_eq!(results[0], results[1], "{src}");
    results.pop().unwrap()
}
#[test]
fn map_helpers_distinguish_missing_from_explicit_nil() {
    let (out,r)=run("let m={\"present\":nil,\"n\":7};print(get(m,\"present\",9),get(m,\"missing\",9),get(m,\"missing\"),has(m,\"present\"),has(m,\"missing\"),get(m,\"n\"));");
    r.unwrap();
    assert_eq!(out, b"nil 9 nil true false 7\n");
}
#[test]
fn default_argument_evaluation_stays_eager() {
    let (out, r) = run("get({\"x\":1},\"x\",assert(false,\"eager\"));");
    assert!(out.is_empty());
    assert_eq!(r.unwrap_err().msg, "eager");
}
#[test]
fn helpers_return_aliases_without_mutating_input() {
    let (out,r)=run("let m={\"x\":[1]};let v=get(m,\"x\");push(v,2);let fallback=[];let x=get(m,\"other\",fallback);push(x,3);print(m,fallback);");
    r.unwrap();
    assert_eq!(out, b"{\"x\": [1, 2]} [3]\n");
}
#[test]
fn join_is_utf8_aware_and_does_not_mutate_lists() {
    let (out,r)=run("let xs=[\"hé\",\"🌍\"];print(join(xs,\" / \"),xs,join([],\"x\"),join([\"one\"],\"ignored\"));");
    r.unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "hé / 🌍 [\"hé\", \"🌍\"]  one\n"
    );
}
#[test]
fn sum_preserves_checked_sequential_numeric_semantics() {
    let (out, r) = run("let xs=[1,2,3];print(sum(xs),sum([]),sum([1,0.5,2]),sum([-2,1]),xs);");
    r.unwrap();
    assert_eq!(out, b"6 0 3.5 -1 [1, 2, 3]\n");
    let (_, r) = run("\nsum([9223372036854775807,1,1.0]);");
    let e = r.unwrap_err();
    assert_eq!(e.msg, "integer overflow");
    assert_eq!((e.span.line, e.span.col), (2, 4));
}
#[test]
fn strict_arity_and_type_failures_match_between_engines() {
    for (src, message) in [
        ("get({});", "get() takes 2 or 3 arguments"),
        ("get({},\"k\",1,2);", "get() takes 2 or 3 arguments"),
        ("get([],\"k\");", "get() expects a map, got list"),
        ("get({},1);", "get() key must be a string, got int"),
        ("has({});", "has() takes 2 argument(s), got 1"),
        ("has([],\"k\");", "has() expects a map, got list"),
        ("has({},nil);", "has() key must be a string, got nil"),
        ("join([]);", "join() takes 2 argument(s), got 1"),
        ("join({},\"\");", "join() expects a list, got map"),
        ("join([],1);", "join() separator must be a string, got int"),
        ("join([1],\"\");", "join() expects string elements, got int"),
        ("sum();", "sum() takes 1 argument(s), got 0"),
        ("sum({});", "sum() expects a list, got map"),
        ("sum([true]);", "sum() expects numeric elements, got bool"),
    ] {
        let (out, r) = run(src);
        assert!(out.is_empty());
        let e = r.unwrap_err();
        assert_eq!(e.kind, Kind::Runtime);
        assert_eq!(e.msg, message);
    }
}
#[test]
fn collection_work_is_bounded_before_iteration() {
    let list = Value::List(Rc::new(RefCell::new(vec![
        Value::Int(0);
        stdlib::MAX_COLLECTION_WORK + 1
    ])));
    let span = Span { line: 7, col: 8 };
    for (name, args) in [
        ("sum", vec![list.clone()]),
        ("join", vec![list, Value::Str("".into())]),
    ] {
        let e = match stdlib::call(name, args, span) {
            Err(e) => e,
            Ok(_) => panic!("work limit bypass"),
        };
        assert_eq!(
            e.msg,
            format!("{name}() element limit exceeded (1000000 elements)")
        );
        assert_eq!(e.span, span);
    }
}
#[test]
fn join_checks_total_utf8_bytes_before_allocation() {
    let text: Rc<str> = "é".repeat(4 * 1024 * 1024).into(); // 8 MiB, two aliases are exactly 16 MiB.
    let list = Value::List(Rc::new(RefCell::new(vec![
        Value::Str(text.clone()),
        Value::Str(text),
    ])));
    let span = Span { line: 3, col: 9 };
    let e = match stdlib::call("join", vec![list.clone(), Value::Str("!".into())], span) {
        Err(e) => e,
        Ok(_) => panic!("byte limit bypass"),
    };
    assert_eq!(e.msg, "join() output limit exceeded (16777216 bytes)");
    assert_eq!(e.span, span);
    let value = stdlib::call("join", vec![list, Value::Str("".into())], span).unwrap();
    let Value::Str(value) = value else {
        panic!("expected string")
    };
    assert_eq!(value.len(), 16 * 1024 * 1024);
}
#[test]
fn builtin_values_remain_first_class_and_policy_controlled() {
    let src = "let f=sum;let g=join;print(f([1,2]),g([\"a\",\"b\"],\"-\"));";
    let (out, r) = run(src);
    r.unwrap();
    assert_eq!(out, b"3 a-b\n");
    for engine in [Engine::Tree, Engine::Vm] {
        let mut out = Vec::new();
        let e = limits::sandbox::run(
            "let f=sum;f([1,2]);",
            &mut out,
            Limits::default(),
            engine,
            limits::sandbox::SandboxPolicy::allow_only(["len"]),
        )
        .unwrap_err();
        assert_eq!(e.msg, "sandbox denied builtin 'sum'");
    }
}
