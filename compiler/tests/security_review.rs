use limits::error::{Kind, LimError};
use limits::interp::{Interpreter, Limits};
use limits::sandbox::{CancellationToken, SandboxPolicy};
use limits::{codegen, lexer, parser, vm::Vm, Engine};
use std::io::{self, Write};

#[test]
fn compiled_programs_do_not_cache_authority_from_prior_invocations() {
    let source="let effects={\"emit\":print};fn use_effect(){effects[\"emit\"](\"allowed\");}use_effect();";
    let ast = parser::parse(lexer::lex(source).unwrap()).unwrap();
    let compiled = codegen::compile(&ast).unwrap();
    for engine in [Engine::Tree, Engine::Vm] {
        let mut permitted = Vec::new();
        match engine {
            Engine::Tree => Interpreter::with_policy(
                &mut permitted,
                Limits::default(),
                SandboxPolicy::default(),
            )
            .run(&ast),
            Engine::Vm => {
                Vm::with_policy(&mut permitted, Limits::default(), SandboxPolicy::default())
                    .run(compiled.clone())
            }
        }
        .unwrap();
        assert_eq!(permitted, b"allowed\n");
        let mut denied = Vec::new();
        let result = match engine {
            Engine::Tree => {
                Interpreter::with_policy(&mut denied, Limits::default(), SandboxPolicy::pure())
                    .run(&ast)
            }
            Engine::Vm => Vm::with_policy(&mut denied, Limits::default(), SandboxPolicy::pure())
                .run(compiled.clone()),
        };
        assert!(denied.is_empty());
        assert_eq!(result.unwrap_err().msg, "sandbox denied builtin 'print'");
    }
}
#[test]
fn builtin_capabilities_survive_function_parameters_maps_and_escaping_closures() {
    let source="fn make(f) {\n    let table={\"effect\":f};\n    fn invoke(){return table[\"effect\"](\"blocked\");}\n    return invoke;\n}\nlet f=make(print);\nf();";
    let mut results = Vec::new();
    for engine in [Engine::Tree, Engine::Vm] {
        let mut out = Vec::new();
        let e = limits::sandbox::run(
            source,
            &mut out,
            Limits::default(),
            engine,
            SandboxPolicy::pure(),
        )
        .unwrap_err();
        assert!(out.is_empty());
        assert_eq!(e.kind, Kind::Runtime);
        assert_eq!(e.msg, "sandbox denied builtin 'print'");
        let col = source
            .lines()
            .nth(2)
            .unwrap()
            .find("(\"blocked\")")
            .unwrap()
            + 1;
        assert_eq!((e.span.line, e.span.col), (3, col));
        results.push(e);
    }
    assert_eq!(results[0], results[1]);
}
struct CancelOnOutput {
    token: CancellationToken,
    bytes: Vec<u8>,
}
impl Write for CancelOnOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        self.token.cancel();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn supervisor_cancellation_stops_an_already_running_invocation() {
    let mut errors: Vec<LimError> = Vec::new();
    for engine in [Engine::Tree, Engine::Vm] {
        let token = CancellationToken::new();
        let mut out = CancelOnOutput {
            token: token.clone(),
            bytes: Vec::new(),
        };
        let policy = SandboxPolicy {
            cancellation: Some(token),
            ..SandboxPolicy::default()
        };
        let e = limits::sandbox::run(
            "print(\"é\");\nprint(\"after\");",
            &mut out,
            Limits::default(),
            engine,
            policy,
        )
        .unwrap_err();
        assert_eq!(out.bytes, "é\n".as_bytes());
        assert_eq!(e.msg, "execution cancelled");
        assert_eq!((e.span.line, e.span.col), (2, 1));
        errors.push(e);
    }
    assert_eq!(errors[0], errors[1]);
}
