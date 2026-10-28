use limits::error::{Kind, LimError};
use limits::interp::{Interpreter, Limits, BUILTINS};
use limits::sandbox::{self, CancellationToken, SandboxPolicy};
use limits::{codegen, lexer, parser, vm::Vm, Engine};
use std::io::{self, Write};

fn run(source: &str, engine: Engine, policy: SandboxPolicy) -> (Vec<u8>, Result<(), LimError>) {
    let mut out = Vec::new();
    let result = sandbox::run(source, &mut out, Limits::default(), engine, policy);
    (out, result)
}

#[test]
fn every_native_identity_is_denied_through_a_map_alias() {
    for name in BUILTINS {
        let source = format!("let aliases={{\"call\":{name}}};aliases[\"call\"]();");
        let mut errors = Vec::new();
        for engine in [Engine::Tree, Engine::Vm] {
            let (out, result) = run(
                &source,
                engine,
                SandboxPolicy::allow_only(Vec::<String>::new()),
            );
            assert!(out.is_empty(), "{name}: {out:?}");
            let error = result.unwrap_err();
            assert_eq!(error.kind, Kind::Runtime);
            // Denial precedes argument validation, including for zero-arity calls.
            assert_eq!(error.msg, format!("sandbox denied builtin '{name}'"));
            errors.push(error);
        }
        assert_eq!(errors[0], errors[1], "{source}");
    }
}

#[test]
fn unknown_allowlist_entries_fail_before_an_otherwise_allowed_effect() {
    for unknown in ["Print", "", "print ", "print\0", "filesystem"] {
        for engine in [Engine::Tree, Engine::Vm] {
            let (out, result) = run(
                "print(\"must not be written\");",
                engine,
                SandboxPolicy::allow_only(["print", unknown]),
            );
            assert!(out.is_empty());
            let error = result.unwrap_err();
            assert_eq!(error.kind, Kind::Runtime);
            assert_eq!(
                error.msg,
                format!("unknown builtin in sandbox policy: '{unknown}'")
            );
            assert_eq!((error.span.line, error.span.col), (1, 1));
        }
    }
}

#[test]
fn rebinding_and_mutating_an_escaped_capture_do_not_change_native_authority() {
    let sources = [
        "let f=len;fn swap(){f=print;}swap();f(\"blocked\");",
        r#"
            fn make(){
                let effect=len;
                fn get(){return effect;}
                fn replace(){effect=print;}
                return {"get":get,"replace":replace};
            }
            let pair=make();
            pair["replace"]();
            pair["get"]()("blocked");
        "#,
        // This argument shape also exercises the VM's fused call optimization.
        "let type=print;let n=0;type(n+1);",
        "fn invoke(type,n){return type(n+1);}invoke(print,0);",
    ];
    for source in sources {
        let tree = run(
            source,
            Engine::Tree,
            SandboxPolicy::allow_only(["len", "type"]),
        );
        let vm = run(
            source,
            Engine::Vm,
            SandboxPolicy::allow_only(["len", "type"]),
        );
        assert_eq!(tree, vm, "{source}");
        assert!(tree.0.is_empty());
        assert_eq!(tree.1.unwrap_err().msg, "sandbox denied builtin 'print'");
    }
    for engine in [Engine::Tree, Engine::Vm] {
        // A name associated with I/O does not restrict an allowed pure builtin.
        let (out, result) = run(
            "let print=type;let n=0;print(n+1);",
            engine,
            SandboxPolicy::allow_only(["type"]),
        );
        result.unwrap();
        assert!(out.is_empty());
    }
}

#[test]
fn cached_fused_calls_use_each_invocations_capabilities() {
    let ast = parser::parse(lexer::lex("let type=print;let n=0;type(n+1);").unwrap()).unwrap();
    let compiled = codegen::compile(&ast).unwrap();
    for allow in [false, true, false, true] {
        let policy = SandboxPolicy::allow_only([if allow { "print" } else { "type" }]);
        let mut out = Vec::new();
        let result = Vm::with_policy(&mut out, Limits::default(), policy).run(compiled.clone());
        if allow {
            result.unwrap();
            assert_eq!(out, b"1\n");
        } else {
            assert_eq!(result.unwrap_err().msg, "sandbox denied builtin 'print'");
            assert!(out.is_empty());
        }
    }
}

struct InterruptedThenShort {
    interruptions: usize,
    bytes: Vec<u8>,
}
impl Write for InterruptedThenShort {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.interruptions > 0 {
            self.interruptions -= 1;
            return Err(io::ErrorKind::Interrupted.into());
        }
        self.bytes.push(bytes[0]);
        Ok(1)
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("the engine must leave flushing to its caller")
    }
}

#[test]
fn interrupted_and_short_writes_preserve_all_utf8_bytes_at_the_exact_cap() {
    for engine in [Engine::Tree, Engine::Vm] {
        let mut out = InterruptedThenShort {
            interruptions: 3,
            bytes: Vec::new(),
        };
        let error = sandbox::run(
            "print(\"é\",\"😀\");\nprint();",
            &mut out,
            Limits::default(),
            engine,
            SandboxPolicy {
                max_output_bytes: Some(8),
                ..SandboxPolicy::default()
            },
        )
        .unwrap_err();
        assert_eq!(out.bytes, "é 😀\n".as_bytes());
        assert_eq!(error.msg, "output limit exceeded (8 bytes)");
        assert_eq!((error.span.line, error.span.col), (2, 6));
    }
}

struct CancelDuringWrite {
    token: CancellationToken,
    calls: usize,
    partial: bool,
    bytes: Vec<u8>,
}
impl Write for CancelDuringWrite {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.partial {
            self.token.cancel();
            self.bytes.push(bytes[0]);
            Ok(1)
        } else if self.calls <= 3 {
            if self.calls == 3 {
                self.token.cancel();
            }
            Err(io::ErrorKind::Interrupted.into())
        } else {
            // Bound the test itself if an Interrupted retry omits a checkpoint.
            Err(io::Error::other("missed cancellation checkpoint"))
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn cancellation_is_checked_between_short_writes_and_interrupted_retries() {
    for partial in [false, true] {
        let mut errors = Vec::new();
        for engine in [Engine::Tree, Engine::Vm] {
            let token = CancellationToken::new();
            let mut out = CancelDuringWrite {
                token: token.clone(),
                calls: 0,
                partial,
                bytes: Vec::new(),
            };
            let error = sandbox::run(
                "\nprint(\"é\");",
                &mut out,
                Limits::default(),
                engine,
                SandboxPolicy {
                    cancellation: Some(token),
                    ..SandboxPolicy::default()
                },
            )
            .unwrap_err();
            assert_eq!(out.calls, if partial { 1 } else { 3 });
            assert_eq!(out.bytes, if partial { vec![0xc3] } else { Vec::new() });
            assert_eq!(error.kind, Kind::Runtime);
            assert_eq!(error.msg, "execution cancelled");
            assert_eq!((error.span.line, error.span.col), (2, 6));
            errors.push(error);
        }
        assert_eq!(errors[0], errors[1]);
    }
}

struct FailAfterPrefix {
    calls: usize,
    bytes: Vec<u8>,
}
impl Write for FailAfterPrefix {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        match self.calls {
            1 => {
                self.bytes.push(bytes[0]);
                Ok(1)
            }
            2 => Err(io::Error::other("sink failed after prefix")),
            _ => {
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn accepted_prefix_counts_across_tree_runs_but_vm_invocations_reset_it() {
    let ast = |source| parser::parse(lexer::lex(source).unwrap()).unwrap();
    let partial = ast("print(\"é\");");
    let small = ast("print(1);");
    let policy = SandboxPolicy {
        max_output_bytes: Some(3),
        ..SandboxPolicy::default()
    };
    let mut out = FailAfterPrefix {
        calls: 0,
        bytes: Vec::new(),
    };
    {
        let mut tree = Interpreter::with_policy(&mut out, Limits::default(), policy.clone());
        assert_eq!(
            tree.run(&partial).unwrap_err().msg,
            "sink failed after prefix"
        );
        // Only the one accepted byte consumed budget; the unwritten suffix did not.
        tree.run(&small).unwrap();
        assert_eq!(
            tree.run(&small).unwrap_err().msg,
            "output limit exceeded (3 bytes)"
        );
    }
    assert_eq!(out.bytes, [0xc3, b'1', b'\n']);
    let mut out = FailAfterPrefix {
        calls: 0,
        bytes: Vec::new(),
    };
    {
        let mut vm = Vm::with_policy(&mut out, Limits::default(), policy);
        let compiled = codegen::compile(&partial).unwrap();
        assert_eq!(
            vm.run(compiled.clone()).unwrap_err().msg,
            "sink failed after prefix"
        );
        vm.run(compiled.clone()).unwrap();
        vm.run(compiled).unwrap();
    }
    assert_eq!(
        out.bytes,
        [vec![0xc3], "é\né\n".as_bytes().to_vec()].concat()
    );
}

#[test]
fn engine_reuse_never_resets_a_supervisors_cancelled_token() {
    let ast = parser::parse(lexer::lex("print(1);").unwrap()).unwrap();
    let compiled = codegen::compile(&ast).unwrap();
    for engine in [Engine::Tree, Engine::Vm] {
        let token = CancellationToken::new();
        let policy = SandboxPolicy {
            cancellation: Some(token.clone()),
            ..SandboxPolicy::default()
        };
        let mut out = Vec::new();
        match engine {
            Engine::Tree => {
                let mut tree = Interpreter::with_policy(&mut out, Limits::default(), policy);
                tree.run(&ast).unwrap();
                token.cancel();
                for _ in 0..3 {
                    assert_eq!(tree.run(&ast).unwrap_err().msg, "execution cancelled");
                }
            }
            Engine::Vm => {
                let mut vm = Vm::with_policy(&mut out, Limits::default(), policy);
                vm.run(compiled.clone()).unwrap();
                token.cancel();
                for _ in 0..3 {
                    assert_eq!(
                        vm.run(compiled.clone()).unwrap_err().msg,
                        "execution cancelled"
                    );
                }
            }
        }
        assert!(token.is_cancelled());
        assert_eq!(out, b"1\n");
    }
}

struct InvalidCount {
    count: usize,
    calls: usize,
}
impl Write for InvalidCount {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        Ok(self.count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn zero_and_oversized_writer_counts_are_terminal_runtime_errors() {
    for count in [0, usize::MAX] {
        let mut errors = Vec::new();
        for engine in [Engine::Tree, Engine::Vm] {
            let mut out = InvalidCount { count, calls: 0 };
            let error = sandbox::run(
                "\nprint(1);",
                &mut out,
                Limits::default(),
                engine,
                SandboxPolicy::restricted(),
            )
            .unwrap_err();
            assert_eq!(out.calls, 1);
            assert_eq!(error.kind, Kind::Runtime);
            assert_eq!((error.span.line, error.span.col), (2, 6));
            let expected = if count == 0 {
                io::Error::from(io::ErrorKind::WriteZero).to_string()
            } else {
                "host writer returned an invalid byte count".to_owned()
            };
            assert_eq!(error.msg, expected);
            errors.push(error);
        }
        assert_eq!(errors[0], errors[1]);
    }
}
