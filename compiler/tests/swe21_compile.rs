//! Wide layouts must preserve observable ordering across fresh compilations.
//! Compile-time measurements live in bench/swe21_compile.jsonl, not assertions.
use limits::error::{Kind, Span};
use limits::{codegen, interp::Interpreter, lexer, parser, vm::Vm};

#[test]
fn wide_layouts_keep_late_capture_binding_and_the_first_runtime_error() {
    let mut source = String::from("let selected=7;\n");
    for i in 0..1500 {
        source.push_str(&format!("let v{i}={i};\n"));
    }
    source.push_str(
        "fn outer(){\n\
         fn read(){return selected;}\n\
         print(read());\n\
         let selected=99;\n\
         print(read());\n\
         if false and missing_short+1 {print(\"wrong\");}\n\
         if true or missing_short+1 {print(\"kept\");}\n\
         if selected<100 {print(\"fused\");}\n\
         print(v0,v750,v1499);\n",
    );
    let error_line = source.lines().count() + 1;
    source.push_str(
        "if false or missing_first+1 {print(\"wrong\");}\n\
         print(missing_second);\n\
         }\n\
         outer();\n",
    );
    let program = parser::parse(lexer::lex(&source).unwrap()).unwrap();
    let mut tree_output = Vec::new();
    let tree_result = Interpreter::new(&mut tree_output).run(&program);
    assert_eq!(tree_output, b"7\n99\nkept\nfused\n0 750 1499\n");
    let error = tree_result.as_ref().unwrap_err();
    assert_eq!(error.kind, Kind::Runtime);
    assert_eq!(error.msg, "undefined variable 'missing_first'");
    assert_eq!(
        error.span,
        Span {
            line: error_line,
            col: 13
        }
    );

    // Each compile creates fresh randomized hash tables. Their lookup order
    // must not change late captures, short circuits, partial stdout, or errors.
    for _ in 0..16 {
        let compiled = codegen::compile(&program).unwrap();
        let mut vm_output = Vec::new();
        let vm_result = Vm::new(&mut vm_output).run(compiled);
        assert_eq!(vm_output, tree_output);
        assert_eq!(vm_result, tree_result);
    }
}
