//! Correctness at the wide-scope/parameter shapes used by phase benchmarks.
//! Timings are measured by bench_pipeline, never asserted on shared CI hosts.
use limits::{codegen, interp::Interpreter, lexer, parser, vm::Vm};

fn parity(source: &str) -> String {
    let ast = parser::parse(lexer::lex(source).unwrap()).unwrap();
    let program = codegen::compile(&ast).unwrap();
    let mut tree = Vec::new();
    let mut vm = Vec::new();
    let tree_result = Interpreter::new(&mut tree).run(&ast);
    let vm_result = Vm::new(&mut vm).run(program);
    assert_eq!(tree_result, vm_result);
    assert_eq!(tree, vm);
    tree_result.unwrap();
    String::from_utf8(tree).unwrap()
}

#[test]
fn thousands_of_declarations_resolve_their_own_slots() {
    let mut source = String::new();
    let mut expected = Vec::new();
    for i in 0..4000 {
        source.push_str(&format!("let v{i}={i};\n"));
    }
    for i in 0..4000 {
        source.push_str(&format!("v{i}=v{i}+1;\n"));
    }
    source.push_str("print(");
    for i in (0..4000).step_by(100) {
        if i != 0 {
            source.push(',');
        }
        source.push_str(&format!("v{i}"));
        expected.push((i + 1).to_string());
    }
    source.push_str(");");
    assert_eq!(parity(&source), expected.join(" ") + "\n");
}

#[test]
fn wide_parent_scopes_are_shared_without_losing_late_capture_binding() {
    let mut source = String::new();
    for i in 0..1000 {
        source.push_str(&format!("let outer{i}={i};\n"));
    }
    for i in 0..100 {
        source.push_str(&format!("fn f{i}(){{return outer{}+late;}}\n", i * 10));
    }
    source.push_str("let late=7; print(f0(),f50(),f99());");
    assert_eq!(parity(&source), "7 507 997\n");
}

#[test]
fn wide_parameter_lists_keep_source_argument_order() {
    let parameters = (0..1000)
        .map(|i| format!("p{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let arguments = (0..1000)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let source = format!("fn f({parameters}){{return p0+p500+p999;}}print(f({arguments}));");
    assert_eq!(parity(&source), "1499\n");
}

#[test]
fn duplicate_parameters_and_redeclarations_keep_existing_slot_semantics() {
    let parameters = vec!["x"; 500].join(",");
    let arguments = (0..500)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let source = format!("fn f({parameters}){{let y=8;let x=x+y;return x;}}print(f({arguments}));");
    // The library intentionally permits this AST; CLI static checking rejects
    // duplicate parameters. Its existing runtime behavior is last argument wins.
    assert_eq!(parity(&source), "507\n");
    assert_eq!(parity("let print=print;let x=1;let x=x+2;print(x);"), "3\n");
}

#[test]
fn concise_agent_report_is_equivalent_without_minification() {
    let manual = include_str!("../../bench/agent_compact/report_manual.lim");
    let compact = include_str!("../../bench/agent_compact/report_helpers.lim");
    assert_eq!(parity(manual), "untitled true search -> read -> answer 9\n");
    assert_eq!(parity(compact), parity(manual));
    // Defaults apply to absent keys, not present nil values, and membership is
    // independent of the stored value's truthiness.
    for replacement in [
        "{\"title\": nil, \"attempt\": false}",
        "{\"title\": \"task\"}",
        "{}",
    ] {
        assert_eq!(
            parity(&manual.replace("{\"attempt\": nil}", replacement)),
            parity(&compact.replace("{\"attempt\": nil}", replacement))
        );
    }
}
#[test]
fn compact_action_selection_preserves_the_existing_example_result() {
    let original = include_str!("../../examples/agent_task.lim");
    let compact = include_str!("../../bench/agent_compact/actions_compact.lim");
    assert_eq!(parity(compact), parity(original));
    assert_eq!(parity(compact), "chosen: search\n");
}
