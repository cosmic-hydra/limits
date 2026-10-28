//! Public embedding contracts across failed invocations and thread boundaries.
use limits::ast::{Expr, Program, Stmt};
use limits::error::{Kind, LimError, Span};
use limits::interp::{Interpreter, Limits};
use limits::sandbox::{CancellationToken, SandboxPolicy};
use limits::{codegen, lexer, parser, resolve, vm::Vm, Engine};
use std::io::{self, Write};
use std::sync::mpsc;
use std::time::Duration;

fn parse(source: &str) -> Program {
    parser::parse(lexer::lex(source).unwrap()).unwrap()
}

#[test]
fn rejected_ast_does_not_mutate_retained_globals_or_charge_steps() {
    let span = Span { line: 19, col: 7 };
    let rejected = vec![
        Stmt::Assign(Expr::Var("n".into(), span), Expr::Int(999), span),
        // Validation covers unreachable bodies before running any statement.
        Stmt::If(
            Expr::Bool(false),
            vec![Stmt::Assign(Expr::Int(0), Expr::Int(1), span)],
            None,
            span,
        ),
    ];
    let compiled_error = codegen::compile(&rejected).err().unwrap();
    assert_eq!(
        resolve::check(&rejected).as_slice(),
        std::slice::from_ref(&compiled_error)
    );
    let mut out = Vec::new();
    {
        let mut tree = Interpreter::with_limits(
            &mut out,
            Limits {
                max_steps: 4,
                max_depth: 1,
            },
        );
        tree.run(&parse("let n=40;")).unwrap();
        assert_eq!(tree.run(&rejected).unwrap_err(), compiled_error);
        tree.run(&parse("n=n+1;print(n);")).unwrap();
        let error = tree.run(&parse("n=n+1;")).unwrap_err();
        assert_eq!(error.msg, "step limit exceeded (4 steps)");
    }
    assert_eq!(out, b"41\n");
    assert_eq!(compiled_error.kind, Kind::Parse);
    assert_eq!(compiled_error.span, span);
}

#[derive(Default)]
struct RecoveringWriter {
    calls: usize,
    flushes: usize,
    bytes: Vec<u8>,
}

impl Write for RecoveringWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.calls == 1 {
            return Err(io::Error::other("host sink temporarily unavailable"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

#[test]
fn nested_writer_error_retains_exported_tree_closure_and_consumed_steps() {
    let failed = parse(
        "let saved=nil;\n\
         fn export(){\n\
           let cell=[40];\n\
           fn read(){cell[0]=cell[0]+1;return cell[0];}\n\
           saved=read;\n\
           print(\"failed\");\n\
         }\n\
         export();",
    );
    let mut out = RecoveringWriter::default();
    {
        let mut tree = Interpreter::with_limits(
            &mut out,
            Limits {
                max_steps: 14,
                max_depth: 1,
            },
        );
        let error = tree.run(&failed).unwrap_err();
        assert_eq!(error.kind, Kind::Runtime);
        assert_eq!(error.msg, "host sink temporarily unavailable");
        assert_eq!(error.span, Span { line: 6, col: 6 });
        // The failed call's depth is gone, but its exported closure and its
        // statement/call charges belong to this stateful interpreter.
        tree.run(&parse("print(saved());")).unwrap();
        assert_eq!(
            tree.run(&parse("let after=1;")).unwrap_err().msg,
            "step limit exceeded (14 steps)"
        );
    }
    assert_eq!(out.bytes, b"41\n");
    assert_eq!(out.calls, 2);
    assert_eq!(out.flushes, 0);
}

#[test]
fn compiled_clones_keep_metadata_and_fresh_globals_after_mutating_errors() {
    let mut ast = parse(
        "let cell=[{\"n\":0}];\n\
         fn next(){cell[0][\"n\"]=cell[0][\"n\"]+1;return cell[0][\"n\"];}\n\
         print(next(),cell);\n\
         print=0;\n\
         assert(false,\"planned failure\");",
    );
    let compiled = codegen::compile(&ast).unwrap();
    ast.clear();
    let metadata = (
        compiled.chunk().instruction_count(),
        compiled.chunk().constant_count(),
        compiled.chunk().spans().to_vec(),
    );
    assert_eq!(metadata.0, metadata.2.len());
    let healthy = codegen::compile(&parse("print(2);")).unwrap();
    let probe = codegen::compile(&parse("print(cell);")).unwrap();
    let mut out = Vec::new();
    {
        let mut vm = Vm::with_limits(
            &mut out,
            Limits {
                max_steps: 10,
                max_depth: 1,
            },
        );
        for _ in 0..12 {
            let error = vm.run(compiled.clone()).unwrap_err();
            assert_eq!(error.kind, Kind::Runtime);
            assert_eq!(error.msg, "planned failure");
            vm.run(healthy.clone()).unwrap();
            assert_eq!(
                vm.run(probe.clone()).unwrap_err().msg,
                "undefined variable 'cell'"
            );
            assert_eq!(
                (
                    compiled.chunk().instruction_count(),
                    compiled.chunk().constant_count(),
                    compiled.chunk().spans().to_vec(),
                ),
                metadata
            );
        }
    }
    assert_eq!(out, b"1 [{\"n\": 1}]\n2\n".repeat(12));
}

struct WorkerWriter {
    ready: mpsc::SyncSender<()>,
    resume: mpsc::Receiver<()>,
    bytes: Vec<u8>,
    calls: usize,
}

impl Write for WorkerWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        self.bytes.extend_from_slice(bytes);
        if self.calls == 1 {
            self.ready.send(()).map_err(io::Error::other)?;
            self.resume
                .recv_timeout(Duration::from_secs(5))
                .map_err(io::Error::other)?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        panic!("the embedding API must leave flushing to its caller")
    }
}

#[test]
fn supervisor_cancels_running_worker_and_reuse_keeps_the_token_cancelled() {
    fn assert_cancelled(error: LimError, span: Span) {
        assert_eq!(error.kind, Kind::Runtime);
        assert_eq!(error.msg, "execution cancelled");
        assert_eq!(error.span, span);
    }

    for engine in [Engine::Tree, Engine::Vm] {
        let token = CancellationToken::new();
        let worker_token = token.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (resume_tx, resume_rx) = mpsc::sync_channel(1);
        // Engines and compiled programs stay on the thread that creates them;
        // only source-independent controls/results cross the thread boundary.
        let worker = std::thread::spawn(move || {
            let program = parse("print(\"started\");\nwhile true {}");
            let policy = SandboxPolicy {
                cancellation: Some(worker_token),
                ..SandboxPolicy::default()
            };
            let mut out = WorkerWriter {
                ready: ready_tx,
                resume: resume_rx,
                bytes: Vec::new(),
                calls: 0,
            };
            let errors = match engine {
                Engine::Tree => {
                    let mut tree = Interpreter::with_policy(&mut out, Limits::default(), policy);
                    [
                        tree.run(&program).unwrap_err(),
                        tree.run(&program).unwrap_err(),
                    ]
                }
                Engine::Vm => {
                    let compiled = codegen::compile(&program).unwrap();
                    let mut vm = Vm::with_policy(&mut out, Limits::default(), policy);
                    [
                        vm.run(compiled.clone()).unwrap_err(),
                        vm.run(compiled).unwrap_err(),
                    ]
                }
            };
            (out.bytes, out.calls, errors)
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        token.cancel();
        resume_tx.send(()).unwrap();
        let (out, calls, [first, reused]) = worker.join().unwrap();
        assert_eq!(out, b"started\n");
        assert_eq!(calls, 1);
        assert_cancelled(first, Span { line: 2, col: 1 });
        assert_cancelled(reused, Span { line: 1, col: 1 });
        assert!(token.is_cancelled());
    }
}
