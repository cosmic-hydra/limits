use std::io::Write;
use std::process::ExitCode;

use limits::interp::{Interpreter, Limits};
use limits::{lexer, parser, resolve};

const USAGE: &str = "usage: lim run <file.lim> [--max-steps N] [--max-depth N] [--engine vm|tree]
       lim check <file.lim>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, path) = match (args.first(), args.get(1)) {
        (Some(c), Some(p)) => (c.as_str(), p.as_str()),
        _ => {
            eprintln!("{}", USAGE);
            return ExitCode::from(2);
        }
    };
    if !path.ends_with(".lim") {
        eprintln!("error: expected a .lim file, got '{}'", path);
        return ExitCode::from(2);
    }

    let mut engine = "tree";
    let mut limits = Limits::default();
    let mut rest = args[2..].iter();
    while let Some(flag) = rest.next() {
        let raw = rest.next();
        if flag == "--engine" {
            match raw.map(String::as_str) {
                Some(v @ ("tree" | "vm")) => engine = v,
                _ => {
                    eprintln!("error: expected --engine vm|tree\n{}", USAGE);
                    return ExitCode::from(2);
                }
            }
            continue;
        }
        let val = raw.and_then(|v| v.parse::<u64>().ok());
        match (flag.as_str(), val) {
            ("--max-steps", Some(n)) => limits.max_steps = n,
            ("--max-depth", Some(n)) => limits.max_depth = n as usize,
            _ => {
                eprintln!("error: bad flag '{}'\n{}", flag, USAGE);
                return ExitCode::from(2);
            }
        }
    }

    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read '{}': {}", path, e);
            return ExitCode::from(2);
        }
    };

    let prog = match lexer::lex(&src).and_then(parser::parse) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}: {}", path, e);
            return ExitCode::FAILURE;
        }
    };
    // Report every static problem at once, before anything runs.
    let problems = resolve::check(&prog);
    if !problems.is_empty() {
        for e in &problems {
            eprintln!("{}: {}", path, e);
        }
        return ExitCode::FAILURE;
    }

    let result = match cmd {
        "check" => Ok(()),
        "run" => {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            let r = if engine == "vm" {
                limits::vm::Vm::with_limits(&mut out, limits).run(limits::codegen::compile(&prog))
            } else {
                Interpreter::with_limits(&mut out, limits).run(&prog)
            };
            let _ = out.flush();
            r
        }
        _ => {
            eprintln!("{}", USAGE);
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}: {}", path, e);
            ExitCode::FAILURE
        }
    }
}
