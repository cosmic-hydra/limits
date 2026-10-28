use limits::error::{Kind, LimError};
use limits::interp::{Interpreter, Limits};
use limits::sandbox::{self, CancellationToken, SandboxPolicy};
use limits::{codegen, lexer, parser, vm::Vm, Engine};
use std::collections::BTreeSet;
use std::time::Duration;

fn run(src: &str, engine: Engine, policy: SandboxPolicy) -> (Vec<u8>, Result<(), LimError>) {
    let mut out = Vec::new();
    let result = sandbox::run(
        src,
        &mut out,
        Limits {
            max_steps: 10000,
            max_depth: 64,
        },
        engine,
        policy,
    );
    (out, result)
}
fn parity(src: &str, policy: SandboxPolicy) -> (Vec<u8>, Result<(), LimError>) {
    let tree = run(src, Engine::Tree, policy.clone());
    assert_eq!(tree, run(src, Engine::Vm, policy), "{src}");
    tree
}
#[test]
fn unrestricted_policy_preserves_existing_programs() {
    let (out, r) = parity(
        "let xs=[];push(xs,1);print(pop(xs),len(xs));",
        SandboxPolicy::default(),
    );
    r.unwrap();
    assert_eq!(out, b"1 0\n");
}
#[test]
fn actual_builtin_capabilities_cannot_be_renamed_or_captured() {
    for source in [
        "let f=print;\nf(\"blocked\");",
        "let fs=[print];fs[0](\"blocked\");",
        "let f=print;fn call(){return f(\"blocked\");}call();",
        "fn get(){return print;}get()(\"blocked\");",
    ] {
        let (out, result) = parity(source, SandboxPolicy::pure());
        assert!(out.is_empty());
        let e = result.unwrap_err();
        assert_eq!(e.kind, Kind::Runtime);
        assert_eq!(e.msg, "sandbox denied builtin 'print'");
        assert!(e.span.col > 0);
    }
    let (_, result) = parity("let f=print;\nf(\"blocked\");", SandboxPolicy::pure());
    let e = result.unwrap_err();
    assert_eq!((e.span.line, e.span.col), (2, 2));
}
#[test]
fn policy_follows_builtin_identity_not_a_shadowing_name() {
    let (out, result) = parity(
        "fn print(){return 7;}assert(print()==7);",
        SandboxPolicy::pure(),
    );
    result.unwrap();
    assert!(out.is_empty());
}
#[test]
fn deny_by_default_allowlist_is_enforced() {
    let policy = SandboxPolicy::allow_only(["assert", "len"]);
    let (_, result) = parity("let xs=[];push(xs,1);", policy);
    assert_eq!(result.unwrap_err().msg, "sandbox denied builtin 'push'");
    let policy = SandboxPolicy {
        allowed_builtins: Some(BTreeSet::new()),
        ..SandboxPolicy::default()
    };
    let (_, result) = parity("len([]);", policy);
    assert_eq!(result.unwrap_err().msg, "sandbox denied builtin 'len'");
}
#[test]
fn invalid_capabilities_fail_closed() {
    let (_, result) = parity("print(1);", SandboxPolicy::allow_only(["filesystem"]));
    let e = result.unwrap_err();
    assert_eq!(e.msg, "unknown builtin in sandbox policy: 'filesystem'");
    assert_eq!((e.span.line, e.span.col), (1, 1));
}
#[test]
fn output_limit_counts_utf8_spaces_and_newlines() {
    let policy = |n| SandboxPolicy {
        max_output_bytes: Some(n),
        ..SandboxPolicy::default()
    };
    let (out, r) = parity("print(\"é\");", policy(3));
    r.unwrap();
    assert_eq!(out, "é\n".as_bytes());
    let (out, r) = parity("print(\"é\");", policy(2));
    assert!(out.is_empty());
    assert_eq!(r.unwrap_err().msg, "output limit exceeded (2 bytes)");
    let (out, r) = parity("print(1,2);", policy(3));
    assert!(out.is_empty());
    assert_eq!(r.unwrap_err().msg, "output limit exceeded (3 bytes)");
    let (out, r) = parity("print();", policy(0));
    assert!(out.is_empty());
    assert_eq!(r.unwrap_err().msg, "output limit exceeded (0 bytes)");
}
#[test]
fn output_limit_is_aggregate_and_preserves_prior_output() {
    let (out, r) = parity(
        "print(1);\nprint(2);",
        SandboxPolicy {
            max_output_bytes: Some(3),
            ..SandboxPolicy::default()
        },
    );
    assert_eq!(out, b"1\n");
    let e = r.unwrap_err();
    assert_eq!(e.msg, "output limit exceeded (3 bytes)");
    assert_eq!((e.span.line, e.span.col), (2, 6));
}
#[test]
fn zero_execution_deadline_is_deterministic() {
    let (out, r) = parity(
        "\nwhile true {}",
        SandboxPolicy {
            timeout: Some(Duration::ZERO),
            ..SandboxPolicy::default()
        },
    );
    assert!(out.is_empty());
    let e = r.unwrap_err();
    assert_eq!(e.msg, "execution deadline exceeded");
    assert_eq!((e.span.line, e.span.col), (2, 1));
}
#[test]
fn cancellation_token_can_be_shared_with_a_supervisor() {
    let token = CancellationToken::new();
    let supervisor = token.clone();
    std::thread::spawn(move || supervisor.cancel())
        .join()
        .unwrap();
    assert!(token.is_cancelled());
    let (out, r) = parity(
        "\nprint(1);",
        SandboxPolicy {
            cancellation: Some(token),
            ..SandboxPolicy::default()
        },
    );
    assert!(out.is_empty());
    let e = r.unwrap_err();
    assert_eq!(e.msg, "execution cancelled");
    assert_eq!((e.span.line, e.span.col), (2, 1));
}
#[test]
fn vm_output_budget_resets_after_success_and_error() {
    let compile =
        |src| codegen::compile(&parser::parse(lexer::lex(src).unwrap()).unwrap()).unwrap();
    let good = compile("print(1);");
    let too_big = compile("print(111);");
    let mut out = Vec::new();
    {
        let mut vm = Vm::with_policy(
            &mut out,
            Limits::default(),
            SandboxPolicy {
                max_output_bytes: Some(2),
                ..SandboxPolicy::default()
            },
        );
        for _ in 0..20 {
            vm.run(good.clone()).unwrap();
            assert!(vm
                .run(too_big.clone())
                .unwrap_err()
                .msg
                .contains("output limit"));
        }
    }
    assert_eq!(out, b"1\n".repeat(20));
}
#[test]
fn direct_tree_calls_preserve_cumulative_output_budget() {
    let program = parser::parse(lexer::lex("print(1);").unwrap()).unwrap();
    let mut out = Vec::new();
    {
        let mut tree = Interpreter::with_policy(
            &mut out,
            Limits::default(),
            SandboxPolicy {
                max_output_bytes: Some(2),
                ..SandboxPolicy::default()
            },
        );
        tree.run(&program).unwrap();
        assert!(tree.run(&program).unwrap_err().msg.contains("output limit"));
    }
    assert_eq!(out, b"1\n");
}
