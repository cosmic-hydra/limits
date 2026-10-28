use limits::semantic::{analyze, SemanticIndex, Symbol};

fn symbol<'a>(index: &'a SemanticIndex, name: &str, kind: &str) -> &'a Symbol {
    index
        .symbols
        .iter()
        .find(|s| s.name == name && s.kind == kind)
        .unwrap()
}
#[test]
fn declaration_and_parameter_locations_are_real_identifier_tokens() {
    let index = analyze("fn add(α, β) { return α + β; }\nlet answer = add(1, 2);").unwrap();
    let add = symbol(&index, "add", "function");
    assert_eq!(
        (
            add.declarations[0].start.line,
            add.declarations[0].start.col
        ),
        (1, 4)
    );
    assert_eq!(add.parameters, vec!["α", "β"]);
    let alpha = symbol(&index, "α", "parameter");
    assert_eq!(
        (
            alpha.declarations[0].start.line,
            alpha.declarations[0].start.col,
            alpha.declarations[0].end.col
        ),
        (1, 8, 9)
    );
    let beta = symbol(&index, "β", "parameter");
    assert_eq!(beta.declarations[0].start.col, 11);
    assert_eq!(index.definition_at(1, 23).unwrap().id, alpha.id);
    assert_eq!(index.definition_at(2, 14).unwrap().id, add.id);
    assert_eq!(index.references_to(add.id)[0].role, "call");
}
#[test]
fn shadowing_and_late_declarations_keep_fallback_candidates() {
    let index = analyze("let x=1; if true { print(x); let x=x+1; print(x); } print(x);").unwrap();
    let xs: Vec<_> = index.symbols.iter().filter(|s| s.name == "x").collect();
    assert_eq!(xs.len(), 2);
    let refs: Vec<_> = index.references.iter().filter(|r| r.name == "x").collect();
    assert_eq!(refs[0].candidates, vec![xs[1].id, xs[0].id]);
    assert_eq!(refs[1].candidates, vec![xs[1].id, xs[0].id]);
    assert_eq!(refs[3].candidates, vec![xs[0].id]);
    assert_eq!(index.definitions_at(1, 26).len(), 2);
}
#[test]
fn closures_report_lexical_captures_and_functions_resolve_forward() {
    let index =
        analyze("fn make(n){fn add(x){return n+x+later;}return add;} let later=3;").unwrap();
    let add = symbol(&index, "add", "function");
    let n = symbol(&index, "n", "parameter");
    let later = symbol(&index, "later", "variable");
    assert_eq!(add.captures, vec![n.id, later.id]);
    assert!(index.diagnostics.is_empty());
    assert_eq!(index.references_to(n.id).len(), 1);
}
#[test]
fn writes_and_index_mutations_have_distinct_roles() {
    let index = analyze("let x=[0]; x[0]=1; x=[2]; print(x);").unwrap();
    let refs: Vec<_> = index.references.iter().filter(|r| r.name == "x").collect();
    assert_eq!(
        refs.iter().map(|r| r.role.as_str()).collect::<Vec<_>>(),
        vec!["read", "write", "read"]
    );
}
#[test]
fn loop_variables_and_completions_are_scoped() {
    let index =
        analyze("let item=99; for item in [1] { print(item); let inner=2; } print(item);").unwrap();
    let loop_var = symbol(&index, "item", "loop-variable");
    assert_eq!(loop_var.declarations[0].start.col, 18);
    let inside = index.completions(1, 35, "i");
    assert_eq!(
        inside.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        vec!["inner", "int", "item"]
    );
    assert_eq!(inside.last().unwrap().id, loop_var.id);
    assert!(index.completions(1, 64, "inner").is_empty());
    assert!(index.definition_at(1, 65).is_none());
}
#[test]
fn maps_comments_strings_empty_blocks_and_else_if_do_not_create_false_symbols() {
    let source = r#"# let fake=1;
let x={"fn ghost(z) {}": 1};
if x["fn ghost(z) {}"] { } else if false { let yes=2; } else { print(x); }
while false {} for i in {} {} print(x);
"#;
    let index = analyze(source).unwrap();
    assert!(!index
        .symbols
        .iter()
        .any(|s| s.name == "fake" || s.name == "ghost" || s.name == "z"));
    assert!(index.diagnostics.is_empty());
    assert_eq!(
        index.scopes.iter().filter(|s| s.kind == "else-if").count(),
        1
    );
}
#[test]
fn unresolved_names_are_indexed_and_diagnostics_are_preserved() {
    let index = analyze("print(missing); never=1; break;").unwrap();
    assert_eq!(index.diagnostics.len(), 3);
    let missing = index
        .references
        .iter()
        .find(|r| r.name == "missing")
        .unwrap();
    assert_eq!(missing.binding, None);
    assert!(missing.candidates.is_empty());
    assert!(index.definition_at(1, 7).is_none());
}
#[test]
fn redeclarations_share_one_binding_but_keep_each_definition() {
    let index = analyze("let x=1;let x=x+1;print(x);").unwrap();
    let x = symbol(&index, "x", "variable");
    assert_eq!(x.declarations.len(), 2);
    assert_eq!(index.symbols.iter().filter(|s| s.name == "x").count(), 1);
    assert_eq!(index.definition_at(1, 13).unwrap().id, x.id);
}
#[test]
fn serialization_is_deterministic_and_exposes_position_contract() {
    let source = "let α=1;print(α);";
    let one = analyze(source).unwrap();
    let two = analyze(source).unwrap();
    assert_eq!(one, two);
    assert_eq!(one.to_json(), two.to_json());
    assert!(one
        .to_json()
        .contains("\"position_encoding\":\"unicode-scalar-1-based\""));
    assert!(one
        .to_json()
        .contains("\"resolution\":\"lexical-candidates\""));
}
#[test]
fn every_example_can_be_indexed() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "lim") {
            let src = std::fs::read_to_string(&path).unwrap();
            let index = analyze(&src).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(
                index
                    .references
                    .iter()
                    .filter(|r| r.binding.is_none())
                    .count(),
                0,
                "{}",
                path.display()
            );
        }
    }
}

#[test]
fn completion_accepts_module_eof_including_empty_source() {
    let index = analyze("let answer=42;").unwrap();
    assert_eq!(index.completions(1, 15, "ans")[0].name, "answer");
    assert!(index.completions(1, 16, "").is_empty());
    let empty = analyze("").unwrap();
    assert!(empty
        .completions(1, 1, "pr")
        .iter()
        .any(|s| s.name == "print"));
}

#[test]
fn validated_expression_forms_replay_in_every_statement_context() {
    let forms = [
        "0",
        "1.2",
        "\"héllo # λ\"",
        "nil",
        "true",
        "a",
        "(a)",
        "[[a]]",
        "{\"k\": a}",
        "{}",
        "[]",
        "f(a)",
        "f()(a)",
        "[f][0](a)",
        "(-a)",
        "-f(a)[b]",
        "not a and b",
        "a or b and c",
        "not not a",
        "(a or b)(c)",
        "a[\"k\"][b]",
        "a..b",
        "a + b * c",
        "\"# fn pretend(x) {}\"",
        "a == b",
        "not a == b",
        "a and not b",
        "{\"x\": {\"a\": [a,b]}}",
        "f({\"x\":a},[b,c])",
        "a[f(b)]",
        "a..b+c",
        "-a..b",
        "a + -b",
        "f(not a,b)",
    ];
    for expression in forms {
        let contexts = [
            format!("let data={expression};"),
            format!("print({expression});"),
            format!("if {expression} {{let q=a;}} else if {expression} {{print(b);}} else {{print(c);}}"),
            format!("for item in {expression} {{fn inner(){{return item;}} print(inner());}}"),
            format!("while {expression} {{let q={expression};break;}}"),
            format!("fn test(p){{return {expression};}}"),
            format!("a[0]={expression};"),
            format!("a={expression};"),
        ];
        for context in contexts {
            let source = format!("let a=1;let b=2;let c=3;fn f(x,y){{return x;}} {context}");
            let index = analyze(&source).unwrap_or_else(|e| panic!("{source}: {e}"));
            assert!(
                index.references.iter().all(|r| r.binding.is_some()),
                "{source}"
            );
            assert!(!index.to_json().is_empty());
        }
    }
}

#[test]
fn index_diagnostics_share_protocol_caps_without_discarding_internal_errors() {
    use limits::error::{Kind, LimError, Span};
    use limits::protocol::{self, MAX_DIAGNOSTICS, MAX_DIAGNOSTIC_MESSAGE_BYTES};
    let mut index = analyze("").unwrap();
    let message = format!("é{}", "界".repeat(MAX_DIAGNOSTIC_MESSAGE_BYTES / 3 + 10));
    let long = LimError::new(Kind::Check, message.clone(), Span { line: 3, col: 5 });
    index.diagnostics.push(long.clone());
    for i in 1..MAX_DIAGNOSTICS + 7 {
        index.diagnostics.push(LimError::new(
            Kind::Check,
            format!("undefined variable 'missing_{i}'"),
            Span {
                line: i + 3,
                col: 1,
            },
        ));
    }
    let wire = index.to_json();
    assert_eq!(wire, index.to_json(), "serialization is deterministic");
    assert_eq!(index.diagnostics.len(), MAX_DIAGNOSTICS + 7);
    assert_eq!(index.diagnostics[0].msg, message);
    assert_eq!(
        wire.matches("\"severity\":\"error\"").count(),
        MAX_DIAGNOSTICS
    );
    assert!(wire.contains(&format!("\"diagnostic_count\":{}", MAX_DIAGNOSTICS + 7)));
    assert!(wire.contains("\"diagnostics_truncated\":true"));
    assert!(wire.contains(&protocol::diagnostic_json(&long)));
    assert!(wire.contains("\"message_truncated\":true"));
    assert!(!wire.contains(&protocol::quote(&message)));
    assert!(!wire.contains("missing_1024"));

    // The nested index uses the same trusted error-kind classification and
    // message truncation metadata as the outer command envelope.
    index.diagnostics = vec![LimError::new(
        Kind::Runtime,
        "LIM_CHECK\",\"ok\":true",
        Span { line: 1, col: 1 },
    )];
    let wire = index.to_json();
    assert!(wire.contains("\"code\":\"LIM_RUNTIME\""));
    assert!(wire.contains("\"diagnostic_count\":1,\"diagnostics_truncated\":false"));
    assert!(wire.contains(&protocol::diagnostic_json(&index.diagnostics[0])));
}
