use limits::{
    error::{Kind, Span},
    lexer,
    semantic::{analyze, SemanticIndex, SourceRange, Symbol},
};
use std::collections::BTreeMap;

// The annotations are an independent source oracle: `label=name` marks a
// declaration, and `inner+outer:name` states the candidates expected at a use.
// Unmarked source (including identifier-shaped strings/comments) must have no
// definition. No lexer/parser output is used to construct expected positions.
struct Fixture {
    source: String,
    declarations: BTreeMap<String, Vec<SourceRange>>,
    queries: Vec<(SourceRange, Vec<String>)>,
}

fn position(prefix: &str) -> Span {
    let normalized = prefix.replace("\r\n", "\n").replace('\r', "\n");
    Span {
        line: normalized.chars().filter(|&c| c == '\n').count() + 1,
        col: normalized.rsplit('\n').next().unwrap().chars().count() + 1,
    }
}

fn fixture(annotated: &str) -> Fixture {
    let mut result = Fixture {
        source: String::new(),
        declarations: BTreeMap::new(),
        queries: Vec::new(),
    };
    let mut rest = annotated;
    while let Some(open) = rest.find('«') {
        result.source.push_str(&rest[..open]);
        rest = &rest[open + '«'.len_utf8()..];
        let close = rest.find('»').expect("closed test annotation");
        let annotation = &rest[..close];
        let declaration = annotation.contains('=');
        let (labels, name) = annotation
            .split_once(if declaration { '=' } else { ':' })
            .expect("annotation labels and name");
        let start = position(&result.source);
        result.source.push_str(name);
        let range = SourceRange {
            start,
            end: position(&result.source),
        };
        if declaration {
            result
                .declarations
                .entry(labels.into())
                .or_default()
                .push(range);
        }
        result
            .queries
            .push((range, labels.split('+').map(str::to_owned).collect()));
        rest = &rest[close + '»'.len_utf8()..];
    }
    result.source.push_str(rest);
    result
}

fn binding<'a>(index: &'a SemanticIndex, fixture: &Fixture, label: &str) -> &'a Symbol {
    index
        .symbols
        .iter()
        .find(|symbol| {
            if let Some(name) = label.strip_prefix('@') {
                symbol.kind == "builtin" && symbol.name == name
            } else {
                symbol.declarations == fixture.declarations[label]
            }
        })
        .unwrap_or_else(|| panic!("missing binding {label}"))
}

fn assert_every_position(index: &SemanticIndex, fixture: &Fixture) {
    let normalized = fixture.source.replace("\r\n", "\n").replace('\r', "\n");
    for (row, text) in normalized.split('\n').enumerate() {
        for col in 1..=text.chars().count() + 1 {
            let point = (row + 1, col);
            let expected = fixture
                .queries
                .iter()
                .find(|(range, _)| {
                    point >= (range.start.line, range.start.col)
                        && point < (range.end.line, range.end.col)
                })
                .map(|(_, labels)| {
                    labels
                        .iter()
                        .map(|label| binding(index, fixture, label).id)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let actual = index
                .definitions_at(point.0, point.1)
                .iter()
                .map(|symbol| symbol.id)
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "definition at {point:?}: {text:?}");
        }
    }
}

#[test]
fn unicode_source_queries_are_exact_across_all_three_line_endings() {
    let annotated = r#"# fn 𐐀sum(参数,café) { let 局部 = 0; }
let «global=café» = "😀 café 𐐀sum 参数 \"fake\"
# fn pretend(局部) {}
";
fn «function=𐐀sum»(«parameter=参数», «shadow=café») {
  let «local=局部» = {"café": «parameter:参数», "𐐀sum": "fake 局部"};
  for «element=元素» in [«shadow+global:café»] {
    «@print:print»(«element:元素», «local:局部», «parameter:参数»);
  }
  return «shadow+global:café»;
}
«@print:print»(«function:𐐀sum»(1, «global:café»)); # café 参数 𐐀sum
"#;
    let mut baseline = None;
    for ending in ["\n", "\r\n", "\r"] {
        let fixture = fixture(&annotated.replace('\n', ending));
        let index = analyze(&fixture.source).unwrap();
        assert!(index.diagnostics.is_empty());
        assert_every_position(&index, &fixture);
        // This also checks scope bounds, references, captures and module EOF.
        if let Some(expected) = &baseline {
            assert_eq!(&index, expected, "line ending {ending:?}");
        } else {
            baseline = Some(index);
        }
    }
}

#[test]
fn redeclarations_and_nested_fallbacks_retain_exact_identifier_ranges() {
    let fixture = fixture(
        "let «global=λ»=1;
fn «function=probe»(«parameter=λ»){
  «@print:print»(«parameter+global:λ»);
  if true {
    «@print:print»(«inner+parameter+global:λ»);
    let «inner=λ»=«inner+parameter+global:λ»+1;
    let «inner=λ»=«inner+parameter+global:λ»+1;
    «@print:print»(«inner+parameter+global:λ»);
  }
}
",
    );
    let index = analyze(&fixture.source).unwrap();
    assert!(index.diagnostics.is_empty());
    assert_every_position(&index, &fixture);
    let inner = binding(&index, &fixture, "inner");
    assert_eq!(inner.declarations.len(), 2);
    assert_eq!(index.references_to(inner.id).len(), 4);
    assert_eq!(
        index
            .references_to(binding(&index, &fixture, "global").id)
            .len(),
        5
    );
}

fn at(source: &str, needle: &str) -> Span {
    position(&source[..source.find(needle).unwrap()])
}

fn complete_ids(index: &SemanticIndex, point: Span, prefix: &str) -> Vec<usize> {
    index
        .completions(point.line, point.col, prefix)
        .iter()
        .map(|symbol| symbol.id)
        .collect()
}

#[test]
fn completion_scope_edges_follow_body_ranges_and_iterables_keep_outer_scope() {
    let fixture = fixture(
        "let «global=shared»=0;
fn «function=worker»(«parameter=input») {
  let «shadow=shared»=1;
  if true {
    let «branch=branch»=2;
    «@print:print»(«shadow+global:shared»,«parameter:input»,«branch:branch»);
  } # after branch
} # after function
for «item=item» in [«global:shared»] {
  let «local=loop_local»=3;
  «@print:print»(«item:item»);
} # after loop
",
    );
    let index = analyze(&fixture.source).unwrap();
    assert!(index.diagnostics.is_empty());
    assert_every_position(&index, &fixture);

    for (label, before, opening, closing) in [
        ("parameter", ") {", "{\n  let shared", "} # after function"),
        ("branch", "true {", "{\n    let branch", "} # after branch"),
        (
            "item",
            "in [shared]",
            "{\n  let loop_local",
            "} # after loop",
        ),
    ] {
        let symbol = binding(&index, &fixture, label);
        assert!(complete_ids(&index, at(&fixture.source, before), &symbol.name).is_empty());
        for needle in [opening, closing] {
            assert_eq!(
                complete_ids(&index, at(&fixture.source, needle), &symbol.name),
                vec![symbol.id],
                "completion at {needle:?}"
            );
        }
        let mut after = at(&fixture.source, closing);
        after.col += 1;
        assert!(complete_ids(&index, after, &symbol.name).is_empty());
    }
    let shadow = binding(&index, &fixture, "shadow");
    assert_eq!(
        complete_ids(&index, at(&fixture.source, "{\n    let branch"), "shared"),
        vec![shadow.id]
    );
    let eof = position(&fixture.source);
    assert_eq!(
        complete_ids(&index, eof, "shared"),
        vec![binding(&index, &fixture, "global").id]
    );
    assert!(index.completions(eof.line, eof.col + 1, "").is_empty());
}

#[test]
fn lexical_parse_and_check_diagnostics_use_source_scalars_and_real_eof() {
    for ending in ["\n", "\r\n", "\r"] {
        let lexical = "# ignored 😀\nlet α=\"😀\"; @".replace('\n', ending);
        let error = lexer::lex(&lexical).unwrap_err();
        assert_eq!(error.kind, Kind::Lex);
        assert_eq!(error.span, at(&lexical, "@"));
        assert_eq!(error.span, Span { line: 2, col: 12 });
        assert!(error.to_string().starts_with("lex error at 2:12:"));
        assert!(limits::protocol::diagnostic_json(&error).contains("\"line\":2,\"col\":12"));

        let incomplete = "let α=\"😀\";\nfn 𐐀f(参数) {\n  return 参数\n".replace('\n', ending);
        let error = analyze(&incomplete).unwrap_err();
        assert_eq!(error.kind, Kind::Parse);
        assert_eq!(error.span, position(&incomplete));
        assert_eq!(error.span, Span { line: 4, col: 1 });

        let unchecked = "let α=\"😀\";\n# hidden 未知\nprint(\"😀\", 未知);".replace('\n', ending);
        let index = analyze(&unchecked).unwrap();
        assert_eq!(index.diagnostics.len(), 1);
        let error = &index.diagnostics[0];
        assert_eq!(error.kind, Kind::Check);
        let start = position(&unchecked[..unchecked.rfind("未知").unwrap()]);
        assert_eq!(error.span, start);
        assert_eq!(start, Span { line: 3, col: 12 });
        let unresolved = index.references.iter().find(|r| r.name == "未知").unwrap();
        assert_eq!(unresolved.range.start, start);
        assert_eq!(unresolved.range.end, Span { line: 3, col: 14 });
        assert!(unresolved.candidates.is_empty());
        for col in start.col..unresolved.range.end.col {
            assert!(index.definitions_at(start.line, col).is_empty());
        }
    }
}
