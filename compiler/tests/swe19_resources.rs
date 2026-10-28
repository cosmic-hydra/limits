use limits::error::Kind;
use limits::interp::Limits;
use limits::{run_with_engine, Engine};
use std::io::{self, Write};

#[derive(Default)]
struct CountOutput {
    bytes: usize,
    calls: usize,
}

impl Write for CountOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes += bytes.len();
        self.calls += 1;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn print_call_byte_limit_includes_separators_and_final_newline() {
    // After 23 iterations, a is 8 MiB - 1 and b is 8 MiB. The first
    // print is exactly 16 MiB including its space and newline; the next
    // exceeds that bound by one byte. Count writes without storing output.
    let source = "let a=\"\";let b=\"x\";for i in 0..23 {a=a+a+\"x\";b=b+b;}\n\
                  print(a,a);\n\
                  print(a,b);";
    let mut errors = Vec::new();
    for engine in [Engine::Tree, Engine::Vm] {
        let mut output = CountOutput::default();
        let error = run_with_engine(source, &mut output, Limits::default(), engine).unwrap_err();
        assert_eq!(error.kind, Kind::Runtime);
        assert_eq!(error.msg, "value rendering limit exceeded");
        assert_eq!((error.span.line, error.span.col), (3, 6));
        assert_eq!(output.bytes, 16 * 1024 * 1024);
        assert_eq!(output.calls, 1);
        errors.push(error);
    }
    assert_eq!(errors[0], errors[1]);
}
