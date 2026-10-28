//! limits: the Limits native compiler. Limits source -> C11 -> native binary.

mod ast;
mod cgen;
mod check;
mod lexer;
mod module;
mod parser;

use std::path::{Path, PathBuf};
use std::process::{exit, Command};
use std::time::Instant;

#[derive(Debug)]
pub struct Error {
    pub kind: &'static str,
    pub line: u32,
    pub col: u32,
    pub msg: String,
    /// Source file for errors outside the root file; empty for the root.
    pub file: String,
}

impl Error {
    pub fn new(kind: &'static str, line: u32, col: u32, msg: String) -> Self {
        Error {
            kind,
            line,
            col,
            msg,
            file: String::new(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if self.file.is_empty() {
            write!(f, "{} error at ", self.kind)?;
        } else {
            write!(f, "{} error in {} at ", self.kind, self.file)?;
        }
        write!(f, "{}:{}: {}", self.line, self.col, self.msg)
    }
}

const USAGE: &str = "usage: limits <build|run|check> file.lim [-o out] [--unchecked] [--max-steps N] [--max-depth N] [--max-mem MB] [--emit-c] [--release|--debug] [--time]";

struct Args {
    cmd: String,
    file: String,
    out: Option<String>,
    opts: cgen::Opts,
    emit_c: bool,
    debug: bool,
    time: bool,
}

fn usage(msg: &str) -> ! {
    eprintln!("limits: {}\n{}", msg, USAGE);
    exit(2)
}

fn parse_args() -> Args {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().unwrap_or_else(|| usage("missing command"));
    if !matches!(cmd.as_str(), "build" | "run" | "check") {
        usage(&format!("unknown command '{}'", cmd));
    }
    let mut a = Args {
        cmd,
        file: String::new(),
        out: None,
        opts: cgen::Opts {
            checked: true,
            max_steps: 1_000_000_000,
            max_depth: 10_000,
            max_mem: 2048 << 20,
            parts: std::thread::available_parallelism().map_or(1, |n| n.get()),
        },
        emit_c: false,
        debug: false,
        time: false,
    };
    let num = |v: Option<String>, flag: &str| -> u64 {
        v.and_then(|s| s.replace('_', "").parse().ok())
            .unwrap_or_else(|| usage(&format!("{} needs a non-negative integer", flag)))
    };
    while let Some(x) = it.next() {
        match x.as_str() {
            "-o" => a.out = Some(it.next().unwrap_or_else(|| usage("-o needs a path"))),
            "--unchecked" => a.opts.checked = false,
            "--max-steps" => a.opts.max_steps = num(it.next(), "--max-steps"),
            "--max-depth" => a.opts.max_depth = num(it.next(), "--max-depth"),
            "--max-mem" => a.opts.max_mem = num(it.next(), "--max-mem").saturating_mul(1 << 20),
            "--emit-c" => a.emit_c = true,
            "--release" => a.debug = false,
            "--debug" => a.debug = true,
            "--time" => a.time = true,
            _ if x.starts_with('-') => usage(&format!("unknown flag '{}'", x)),
            _ if a.file.is_empty() => a.file = x,
            _ => usage("only one input file is supported"),
        }
    }
    if a.file.is_empty() {
        usage("missing input file");
    }
    if a.emit_c {
        a.opts.parts = 1;
    }
    a
}

/// Lex, parse, check and generate C. Prints errors and exits 1 on failure.
fn frontend(a: &Args) -> Vec<String> {
    let fail = |errs: Vec<Error>| -> ! {
        for e in errs {
            eprintln!("{}", e);
        }
        exit(1)
    };
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let mut mods = module::load(Path::new(&a.file)).unwrap_or_else(|e| fail(e));
    let t_parse = ms(t);
    let t = Instant::now();
    let (prog, structs) = check::check(&mut mods).unwrap_or_else(|e| fail(e));
    let t_check = ms(t);
    let t = Instant::now();
    let out = if a.cmd == "check" {
        Vec::new()
    } else {
        cgen::generate(&prog, &structs, &a.opts)
    };
    if a.time {
        eprintln!(
            "  lex+parse: {:.2} ms, check: {:.2} ms, codegen: {:.2} ms, C files: {}",
            t_parse,
            t_check,
            ms(t),
            out.len()
        );
    }
    // The process exits soon; skip freeing the AST (measurable on big inputs).
    std::mem::forget((mods, prog, structs));
    out
}

/// Compile the C translation units (in parallel when there are several) and
/// link them into `out`.
fn cc(tus: &[String], out: &Path, debug: bool) {
    let compiler = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let opt = if debug {
        ["-O0", "-g"]
    } else {
        ["-O2", "-DNDEBUG"]
    };
    let cmd = || {
        let mut c = Command::new(&compiler);
        c.args(["-std=gnu11", "-fwrapv", "-w"]).args(opt);
        c
    };
    let die = |e: &dyn std::fmt::Display| -> ! {
        eprintln!("limits: {}", e);
        exit(2)
    };
    let srcs: Vec<PathBuf> = (0..tus.len())
        .map(|i| out.with_extension(format!("{}.c", i)))
        .collect();
    for (p, src) in srcs.iter().zip(tus) {
        std::fs::write(p, src)
            .unwrap_or_else(|e| die(&format!("cannot write '{}': {}", p.display(), e)));
    }
    let objs: Vec<PathBuf> = srcs.iter().map(|p| p.with_extension("o")).collect();
    let run = |mut c: Command| match c.status() {
        Ok(s) => s.success(),
        Err(e) => die(&format!("cannot run C compiler '{}': {}", compiler, e)),
    };
    let ok = if tus.len() == 1 {
        let mut c = cmd();
        c.arg("-o").arg(out).arg(&srcs[0]).arg("-lm");
        run(c)
    } else {
        let kids: Vec<_> = srcs
            .iter()
            .zip(&objs)
            .map(|(s, o)| {
                cmd()
                    .arg("-c")
                    .arg("-o")
                    .arg(o)
                    .arg(s)
                    .spawn()
                    .unwrap_or_else(|e| {
                        die(&format!("cannot run C compiler '{}': {}", compiler, e))
                    })
            })
            .collect();
        let mut all = true;
        for mut k in kids {
            all &= k.wait().is_ok_and(|s| s.success());
        }
        let mut c = cmd();
        c.arg("-o").arg(out).args(&objs).arg("-lm");
        all && run(c)
    };
    for p in srcs.iter().chain(&objs) {
        let _ = std::fs::remove_file(p);
    }
    if !ok {
        die(&format!(
            "C compiler '{}' failed (this is a limits bug)",
            compiler
        ));
    }
}

fn main() {
    let a = parse_args();
    let t0 = Instant::now();
    // The frontend recurses over the syntax tree; the parser caps nesting and
    // a large (lazily committed) stack makes that cap safe in every pass.
    let c_src = std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn_scoped(s, || frontend(&a))
            .expect("spawn frontend thread")
            .join()
            .unwrap_or_else(|_| exit(101))
    });
    let t_front = t0.elapsed();
    if a.cmd == "check" {
        if a.time {
            eprintln!("frontend: {:.2} ms", t_front.as_secs_f64() * 1e3);
        }
        return;
    }
    if a.emit_c {
        match &a.out {
            Some(o) => std::fs::write(o, &c_src[0]).unwrap_or_else(|e| {
                eprintln!("limits: cannot write '{}': {}", o, e);
                exit(2)
            }),
            None => print!("{}", c_src[0]),
        }
        return;
    }
    let tmp_dir;
    let out: PathBuf = if a.cmd == "run" {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        tmp_dir = std::env::temp_dir().join(format!("limits-{}-{}", std::process::id(), nanos));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        tmp_dir.join("prog")
    } else {
        tmp_dir = PathBuf::new();
        PathBuf::from(a.out.clone().unwrap_or_else(|| {
            Path::new(&a.file)
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        }))
    };
    let t1 = Instant::now();
    cc(&c_src, &out, a.debug);
    if a.time {
        eprintln!(
            "frontend: {:.2} ms, cc: {:.2} ms",
            t_front.as_secs_f64() * 1e3,
            t1.elapsed().as_secs_f64() * 1e3
        );
    }
    if a.cmd == "run" {
        let status = Command::new(&out).status();
        let _ = std::fs::remove_dir_all(&tmp_dir);
        match status {
            Ok(s) => {
                use std::os::unix::process::ExitStatusExt;
                if let Some(sig) = s.signal() {
                    eprintln!("limits: program killed by signal {}", sig);
                    exit(128 + sig);
                }
                exit(s.code().unwrap_or(1))
            }
            Err(e) => {
                eprintln!("limits: cannot run program: {}", e);
                exit(2)
            }
        }
    }
}
