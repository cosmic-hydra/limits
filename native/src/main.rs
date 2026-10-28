//! limits: the Limits native compiler. Limits source -> C11 -> native binary.

mod ast;
mod cgen;
mod check;
mod diag;
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
    /// Short actionable suggestion; empty if none.
    pub hint: String,
}

impl Error {
    pub fn new(kind: &'static str, line: u32, col: u32, msg: String) -> Self {
        Error {
            kind,
            line,
            col,
            msg,
            file: String::new(),
            hint: String::new(),
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
        write!(f, "{}:{}: {}", self.line, self.col, self.msg)?;
        if !self.hint.is_empty() {
            write!(f, "\n  hint: {}", self.hint)?;
        }
        Ok(())
    }
}

const USAGE: &str = "usage: limits <build|run|check|test> file.lim [-o out] [--unchecked] [--max-steps N] [--max-depth N] [--max-mem MB] [--emit-c] [--release|--debug] [--time] [--json] [--allow fs.read=DIR|fs.write=DIR|env=NAME]... [-- program args]";

struct Args {
    cmd: String,
    file: String,
    out: Option<String>,
    opts: cgen::Opts,
    emit_c: bool,
    debug: bool,
    time: bool,
    json: bool,
    /// Arguments after `--`, passed to the program by `run`.
    prog_args: Vec<String>,
}

fn usage(msg: &str) -> ! {
    eprintln!("limits: {}\n{}", msg, USAGE);
    exit(2)
}

fn parse_args() -> Args {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().unwrap_or_else(|| usage("missing command"));
    if !matches!(cmd.as_str(), "build" | "run" | "check" | "test") {
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
            json: false,
            tests: false,
            allow_read: Vec::new(),
            allow_write: Vec::new(),
            allow_env: Vec::new(),
        },
        emit_c: false,
        debug: false,
        time: false,
        json: false,
        prog_args: Vec::new(),
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
            "--json" => a.json = true,
            "--allow" => {
                let g = it.next().unwrap_or_default();
                let (cap, v) = g.split_once('=').unwrap_or_else(|| {
                    usage("--allow needs fs.read=DIR, fs.write=DIR or env=NAME")
                });
                let dir = || match std::fs::canonicalize(v) {
                    Ok(p) => p.to_string_lossy().into_owned(),
                    Err(e) => usage(&format!("--allow {}: '{}': {}", cap, v, e)),
                };
                match cap {
                    "fs.read" => a.opts.allow_read.push(dir()),
                    "fs.write" => a.opts.allow_write.push(dir()),
                    "env" => a.opts.allow_env.push(v.to_string()),
                    _ => usage(&format!("--allow: unknown grant '{}'", cap)),
                }
            }
            "--" => {
                a.prog_args = it.by_ref().collect();
            }
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
    a.opts.json = a.json;
    a.opts.tests = a.cmd == "test";
    a
}

/// Lex, parse, check and generate C. Prints errors and exits 1 on failure.
/// Also returns the display names of the inline tests (in `test` mode).
fn frontend(a: &Args) -> (Vec<String>, Vec<String>) {
    let fail = |errs: Vec<Error>| -> ! {
        for e in errs {
            if a.json {
                let file = if e.file.is_empty() { &a.file } else { &e.file };
                eprintln!(
                    "{}",
                    diag::json(e.kind, file, e.line, e.col, &e.msg, &e.hint)
                );
            } else {
                eprintln!("{}", e);
            }
        }
        exit(1)
    };
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let mut mods = module::load(Path::new(&a.file)).unwrap_or_else(|e| fail(e));
    let t_parse = ms(t);
    let t = Instant::now();
    let (prog, structs) = check::check(&mut mods, a.cmd != "test").unwrap_or_else(|e| fail(e));
    let t_check = ms(t);
    let t = Instant::now();
    let out = if a.cmd == "check" {
        Vec::new()
    } else {
        cgen::generate(&prog, &structs, &a.opts, &a.file)
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
    let tests = prog.funcs.iter().filter_map(|f| f.test.clone()).collect();
    // The process exits soon; skip freeing the AST (measurable on big inputs).
    std::mem::forget((mods, prog, structs));
    (out, tests)
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
        c.args([
            "-std=gnu11",
            "-fwrapv",
            "-Wall",
            "-Wextra",
            "-Wno-infinite-recursion",
        ])
        .args(opt);
        // Generated C must be warning-free; debug builds of limits (and so
        // `cargo test`) turn warnings into errors to enforce it.
        if cfg!(debug_assertions) {
            c.arg("-Werror");
        }
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
    let (c_src, tests) = std::thread::scope(|s| {
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
    let out: PathBuf = if a.cmd != "build" {
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
    if a.cmd == "test" {
        run_tests(&out, &tests, a.json);
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return;
    }
    if a.cmd == "run" {
        let status = Command::new(&out).args(&a.prog_args).status();
        let _ = std::fs::remove_dir_all(&tmp_dir);
        match status {
            Ok(s) => exit(exit_code(s, a.json)),
            Err(e) => {
                eprintln!("limits: cannot run program: {}", e);
                exit(2)
            }
        }
    }
}

/// Exit code for a finished program; reports death by signal.
fn exit_code(s: std::process::ExitStatus, json: bool) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    let Some(sig) = s.signal() else {
        return s.code().unwrap_or(1);
    };
    let m = format!("program killed by signal {}", sig);
    if json {
        eprintln!("{}", diag::json("runtime", "", 0, 0, &m, ""));
    } else {
        eprintln!("limits: {}", m);
    }
    128 + sig
}

/// Wall-clock limit per test process, so a test built with `--unchecked`
/// (no step budget) that never ends cannot hang the whole run.
const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Run every test in its own process (`prog <index>`), so a test that fails,
/// aborts, exhausts a budget or times out cannot affect the others.
fn run_tests(prog: &Path, names: &[String], json: bool) {
    use std::io::{Read, Write};
    use std::os::unix::process::ExitStatusExt;
    use std::process::Stdio;
    let mut out = std::io::stdout().lock();
    // Names are escaped (as in a JSON string, without the quotes) so a test
    // name cannot forge result lines. A closed stdout ends the run quietly.
    let mut say = |line: String| {
        if writeln!(out, "{}", line).and_then(|_| out.flush()).is_err() {
            exit(2)
        }
    };
    let mut failed = 0;
    for (i, name) in names.iter().enumerate() {
        let shown = diag::json_str(name);
        let shown = &shown[1..shown.len() - 1];
        let child = Command::new(prog)
            .arg(i.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = child.unwrap_or_else(|e| {
            eprintln!("limits: cannot run tests: {}", e);
            exit(2)
        });
        let mut pipe = child.stderr.take().expect("piped stderr");
        let reader = std::thread::spawn(move || {
            let mut v = Vec::new();
            let _ = pipe.read_to_end(&mut v);
            v
        });
        let t0 = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if t0.elapsed() < TEST_TIMEOUT => {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let err = reader.join().unwrap_or_default();
        let why = match status {
            Some(s) if s.success() => {
                say(format!("ok {}", shown));
                continue;
            }
            None => format!("timed out after {} s", TEST_TIMEOUT.as_secs()),
            Some(s) => {
                let err = String::from_utf8_lossy(&err);
                let first = err.lines().next().unwrap_or("").to_string();
                match (first.is_empty(), s.signal()) {
                    (false, _) => first,
                    (true, Some(sig)) => format!("killed by signal {}", sig),
                    (true, None) => format!("exit code {}", s.code().unwrap_or(1)),
                }
            }
        };
        let why = if json && !why.starts_with('{') {
            diag::json("runtime", "", 0, 0, &why, "")
        } else {
            why
        };
        failed += 1;
        say(format!("FAIL {}: {}", shown, why));
    }
    say(format!(
        "{} passed, {} failed",
        names.len() - failed,
        failed
    ));
    if failed > 0 {
        exit(1)
    }
}
