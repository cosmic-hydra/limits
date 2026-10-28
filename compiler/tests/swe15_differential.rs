//! Deterministic compositions around optimizer and continuation boundaries.
//! Every comparison includes partial stdout and every field of LimError.
use limits::ast::Program;
use limits::bytecode::CompiledProgram;
use limits::error::{Kind, LimError};
use limits::interp::{Interpreter, Limits};
use limits::{codegen, lexer, parser, vm::Vm};

type Outcome = (Vec<u8>, Result<(), LimError>);

fn execute(program: &Program, compiled: &CompiledProgram, limits: Limits, vm: bool) -> Outcome {
    let mut stdout = Vec::new();
    let result = if vm {
        Vm::with_limits(&mut stdout, limits).run(compiled.clone())
    } else {
        Interpreter::with_limits(&mut stdout, limits).run(program)
    };
    (stdout, result)
}

fn compare(program: &Program, compiled: &CompiledProgram, source: &str, limits: Limits) -> Outcome {
    let tree = execute(program, compiled, limits, false);
    let vm = execute(program, compiled, limits, true);
    assert_eq!(tree, vm, "source:\n{source}\nlimits: {limits:?}");
    tree
}

// Parsing/compilation happen once per generated source. Sweeping every step
// prefix catches differences hidden by a successful run or by a later error.
fn sweep(source: &str) {
    let program = parser::parse(lexer::lex(source).unwrap()).unwrap();
    let compiled = codegen::compile(&program).unwrap();
    let complete = Limits {
        max_steps: 512,
        max_depth: 16,
    };
    let terminal = compare(&program, &compiled, source, complete);
    if let Err(error) = &terminal.1 {
        assert_eq!(error.kind, Kind::Runtime, "{source}\n{error}");
        assert!(
            !error.msg.contains("step limit"),
            "unbounded corpus: {source}"
        );
    }
    let mut reached_terminal = false;
    for steps in 0..=complete.max_steps {
        let outcome = compare(
            &program,
            &compiled,
            source,
            Limits {
                max_steps: steps,
                ..complete
            },
        );
        if outcome == terminal {
            reached_terminal = true;
            break;
        }
    }
    assert!(reached_terminal, "step sweep did not complete: {source}");
    for depth in [0, 1, 2, 3] {
        let _ = compare(
            &program,
            &compiled,
            source,
            Limits {
                max_depth: depth,
                ..complete
            },
        );
    }
}

#[test]
fn generated_condition_compositions_preserve_short_circuiting() {
    // LHSs distinguish falsy nil/false from truthy zero/empty containers, and
    // record whether evaluating a condition reaches a call. RHSs include the
    // exact variable/integer form used by branch and call fusion.
    let left = [
        "false",
        "nil",
        "true",
        "0",
        "[]",
        "probe(false)",
        "probe(0)",
        "n < 1",
    ];
    let right = [
        "n < 1",
        "n + 1",
        "n / 0",
        "missing + 1",
        "probe(nil)",
        "probe(n + 1)",
        "{\"k\": false}[\"k\"]",
        "[n][0]",
    ];
    for (i, lhs) in left.iter().enumerate() {
        // The three offsets vary the operand pairing deterministically without
        // producing a large Cartesian product of equivalent scalar cases.
        for offset in [0, 1, 3] {
            let rhs = right[(i + offset) % right.len()];
            let expressions = [
                format!("({lhs}) and ({rhs})"),
                format!("({lhs}) or ({rhs})"),
                format!("not (({lhs}) and ({rhs}))"),
                format!("(({lhs}) or ({rhs})) and (n < 1)"),
                format!("(({lhs}) and ({rhs})) or (n < 1)"),
            ];
            for expression in expressions {
                for body in [
                    format!(
                        "if {expression} {{push(trace, \"then\");}} \
                         else {{push(trace, \"else\");}}"
                    ),
                    format!(
                        "while {expression} {{\n\
                         push(trace, n); n = n + 1;\n\
                         if n >= 1 {{break;}}\n\
                         }}"
                    ),
                ] {
                    sweep(&format!(
                        "let trace = [];\n\
                         let n = -1;\n\
                         fn probe(v) {{push(trace, v); return v;}}\n\
                         {body}\n\
                         print(trace, n);"
                    ));
                }
            }
        }
    }
}

#[test]
fn generated_calls_and_index_assignments_preserve_error_order() {
    let functions = [
        "fn target(n) {return n + 1;}",
        "fn target(n) {let x = n; return x + 1;}",
        "fn target(n) {fn read() {return n;} return read();}",
        "fn target() {return 9;}",
        "fn target(n, n) {return n;}",
        "let target = 1;",
        "let target = len;",
    ];
    let arguments = [
        "n + 1",
        "n / 0",
        "missing + 1",
        "probe(n + 1)",
        "(probe(0) or n) + 1",
    ];
    for (i, function) in functions.iter().enumerate() {
        for offset in 0..arguments.len() {
            let argument = arguments[(i + offset) % arguments.len()];
            for callee in ["target", "choose()", "missing_target"] {
                sweep(&format!(
                    "let trace = [];\n\
                     let n = 2;\n\
                     fn probe(v) {{push(trace, v); return v;}}\n\
                     {function}\n\
                     fn choose() {{push(trace, \"callee\"); return target;}}\n\
                     print(\"before\");\n\
                     print({callee}(\n  {argument}\n));\n\
                     print(trace);"
                ));
            }
        }
    }

    let targets = [
        "items[probe(0)]",
        "choose()[probe(0)]",
        "choose()[missing]",
        "missing_items[probe(0)]",
        "nil[probe(0)]",
        "items[probe(9)]",
    ];
    let values = ["probe(7)", "n / 0", "missing + 1"];
    for target in targets {
        for value in values {
            sweep(&format!(
                "let trace = [];\n\
                 let n = 2;\n\
                 let items = [0];\n\
                 let alias = {{\"items\": items}};\n\
                 fn probe(v) {{print(v); push(trace, v); return v;}}\n\
                 fn choose() {{print(\"object\"); return alias[\"items\"];}}\n\
                 {target} = {value};\n\
                 print(items, trace);"
            ));
        }
    }
}

#[test]
fn generated_closures_aliases_and_loop_exits_keep_pending_values() {
    for start in 0..4 {
        for stop in 0..3 {
            sweep(&format!(
                "fn make(n) {{\n\
                   let box = {{\"items\": [n]}};\n\
                   fn change(v) {{box[\"items\"][0] = v; return v;}}\n\
                   fn read() {{return box[\"items\"][0];}}\n\
                   return [change, read, box];\n\
                 }}\n\
                 fn select(n) {{\n\
                   let cells = [];\n\
                   for i in 0..3 {{\n\
                     if i == {stop} {{continue;}}\n\
                     let pair = make(n + 1);\n\
                     let alias = pair[2][\"items\"];\n\
                     alias[0] = i;\n\
                     push(cells, pair);\n\
                     let j = 0;\n\
                     while j < 2 {{\n\
                       j = j + 1;\n\
                       if j == 1 {{continue;}}\n\
                       if i == 2 {{return [99, cells, pair[0](n + 1), pair[1]()];}}\n\
                       break;\n\
                     }}\n\
                   }}\n\
                   return [88, cells];\n\
                 }}\n\
                 let selected = select({start});\n\
                 print(selected[0], len(selected[1]));\n\
                 for pair in selected[1] {{\n\
                   print([pair[1](), pair[0]({start} + 1), pair[1]()], pair[2]);\n\
                 }}"
            ));
        }
    }
}
