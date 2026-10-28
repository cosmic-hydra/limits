//! limits: the Limits native compiler. Limits source -> C11 -> native binary.

mod assist;
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
    /// Span, expected/found, alternatives and fixes (boxed: errors are
    /// passed around in `Result`s and should stay small).
    pub more: Box<diag::More>,
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
            more: Box::default(),
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

const USAGE: &str = "usage: limits <build|run|check|test|fix> file.lim | limits check --stdin [--partial] [--expect] [--json] [-o out] [--unchecked] [--max-steps N] [--max-depth N] [--max-mem MB] [--emit-c] [--release|--debug] [--cc tcc|cc|PATH] [--time] [--json] [--leak-check] [--show-perms] [--timeout SEC] [--no-sandbox] [--pass-env NAME]... [--allow fs.read=DIR|fs.write=DIR|env=NAME]... [-- program args]";

/// The C compiler to use and how to drive it.
#[derive(Debug, Clone)]
enum Compiler {
    /// TinyCC (fast compiles, no optimisation): no `-O`, needs the shim.
    Tcc(PathBuf),
    /// A gcc-compatible compiler (`cc`, `gcc`, `clang`, `$CC`).
    Gcc(String),
}

/// TinyCC to use by default: `$LIMITS_TCC`, else `tools/tcc/bin/tcc` in a
/// directory above the limits executable (the repository layout).
fn find_tcc() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LIMITS_TCC").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .map(|d| d.join("tools/tcc/bin/tcc"))
        .find(|p| p.is_file())
}

/// `--cc` (or `$LIMITS_CC`) if given; else tcc for `run`, `test` and
/// `--debug` when it is available, and `$CC` (default `cc`) otherwise.
fn pick_compiler(a: &Args) -> Compiler {
    let sys = || Compiler::Gcc(std::env::var("CC").unwrap_or_else(|_| "cc".into()));
    let want =
        a.cc.clone()
            .or_else(|| std::env::var("LIMITS_CC").ok().filter(|s| !s.is_empty()));
    match want.as_deref() {
        Some("tcc") => Compiler::Tcc(find_tcc().unwrap_or_else(|| PathBuf::from("tcc"))),
        Some("cc") => sys(),
        Some(other) => Compiler::Gcc(other.to_string()),
        None if a.release || (a.cmd == "build" && !a.debug) => sys(),
        None => find_tcc().map_or_else(sys, Compiler::Tcc),
    }
}

struct Args {
    cmd: String,
    file: String,
    out: Option<String>,
    opts: cgen::Opts,
    emit_c: bool,
    debug: bool,
    /// `--release` given explicitly.
    release: bool,
    /// `--cc NAME`.
    cc: Option<String>,
    /// `--show-perms`: print the capability manifest instead of running.
    show_perms: bool,
    time: bool,
    json: bool,
    /// `check --stdin`: read the root file's text from stdin.
    stdin: bool,
    /// `check --partial`: the input may be an unfinished program prefix.
    partial: bool,
    /// `check --expect`: also describe what may come next at the end.
    expect: bool,
    /// Arguments after `--`, passed to the program by `run`.
    prog_args: Vec<String>,
    /// `--no-sandbox`: run the program without limits-sandbox.
    no_sandbox: bool,
    /// `--timeout SEC`: wall-clock (and CPU) limit for `run`/`test`.
    timeout: u64,
    /// `--pass-env NAME`: variables the sandbox passes to a `uses env` program.
    pass_env: Vec<String>,
}

fn usage(msg: &str) -> ! {
    eprintln!("limits: {}\n{}", msg, USAGE);
    exit(2)
}

fn parse_args() -> Args {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().unwrap_or_else(|| usage("missing command"));
    if !matches!(cmd.as_str(), "build" | "run" | "check" | "test" | "fix") {
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
            leak_check: false,
            tcc: false,
            allow_read: Vec::new(),
            allow_write: Vec::new(),
            allow_env: Vec::new(),
        },
        emit_c: false,
        debug: false,
        release: false,
        cc: None,
        show_perms: false,
        time: false,
        json: false,
        stdin: false,
        partial: false,
        expect: false,
        prog_args: Vec::new(),
        no_sandbox: false,
        timeout: 60,
        pass_env: Vec::new(),
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
            "--release" => (a.debug, a.release) = (false, true),
            "--debug" => (a.debug, a.release) = (true, false),
            "--cc" => {
                a.cc = Some(
                    it.next()
                        .unwrap_or_else(|| usage("--cc needs tcc, cc or a compiler path")),
                )
            }
            "--show-perms" => a.show_perms = true,
            "--time" => a.time = true,
            "--json" => a.json = true,
            "--stdin" => a.stdin = true,
            "--partial" => a.partial = true,
            "--expect" => (a.expect, a.partial) = (true, true),
            "--leak-check" => a.opts.leak_check = true,
            "--no-sandbox" => a.no_sandbox = true,
            "--timeout" => a.timeout = num(it.next(), "--timeout").max(1),
            "--pass-env" => {
                let n = it
                    .next()
                    .unwrap_or_else(|| usage("--pass-env needs a NAME"));
                a.pass_env.push(env_name("--pass-env", n));
            }
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
                    "env" => a
                        .opts
                        .allow_env
                        .push(env_name("--allow env", v.to_string())),
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
    if (a.stdin || a.partial) && a.cmd != "check" {
        usage("--stdin, --partial and --expect work with 'check' only");
    }
    if a.expect && !a.json {
        usage("--expect needs --json");
    }
    if a.stdin {
        if !a.file.is_empty() {
            usage("--stdin takes no input file");
        }
        a.file = "<stdin>".into();
    }
    if a.file.is_empty() {
        usage("missing input file");
    }
    if a.emit_c {
        a.opts.parts = 1;
    }
    a.opts.json = a.json;
    a.opts.tests = a.cmd == "test";
    a.opts.tcc = matches!(pick_compiler(&a), Compiler::Tcc(_));
    a
}

/// Lex, parse, check and generate C. Prints errors and exits 1 on failure.
/// Also returns the display names of the inline tests (in `test` mode).
fn frontend(a: &Args) -> (Vec<String>, Vec<String>, Vec<String>) {
    let fail = |errs: Vec<Error>| -> ! {
        let src = std::fs::read(&a.file)
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned());
        emit_errors(errs, &a.file, src.as_deref(), a.json, false);
        exit(1)
    };
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let mut mods = module::load(Path::new(&a.file)).unwrap_or_else(|e| fail(e));
    let t_parse = ms(t);
    let t = Instant::now();
    let (prog, structs) = check::check(&mut mods, a.cmd != "test").unwrap_or_else(|e| fail(e));
    let t_check = ms(t);
    if a.show_perms {
        show_perms(a, &mods);
        exit(0);
    }
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
    let caps = mods[0].prog.uses.iter().map(|u| u.0.clone()).collect();
    // The process exits soon; skip freeing the AST (measurable on big inputs).
    std::mem::forget((mods, prog, structs));
    (out, tests, caps)
}

/// Directory that module paths in errors are relative to.
fn base_dir(file: &str) -> PathBuf {
    if file == "<stdin>" {
        return std::env::current_dir().unwrap_or_default();
    }
    std::fs::canonicalize(file)
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

/// Print diagnostics: prose, or one JSON object per line (with span, source
/// line, expected/found, alternatives and fixes). To stdout or stderr.
fn emit_errors(errs: Vec<Error>, root: &str, root_src: Option<&str>, json: bool, stdout: bool) {
    use std::io::Write;
    let base = base_dir(root);
    let mut cache: std::collections::HashMap<String, Option<String>> = Default::default();
    let mut out = String::new();
    for mut e in errs {
        if json {
            let src = if e.file.is_empty() {
                root_src.map(str::to_string)
            } else {
                cache
                    .entry(e.file.clone())
                    .or_insert_with(|| std::fs::read_to_string(base.join(&e.file)).ok())
                    .clone()
            };
            diag::enrich(&mut e, src.as_deref());
            let file = if e.file.is_empty() { root } else { &e.file };
            out.push_str(&diag::json_err(&e, file, src.as_deref()));
        } else {
            out.push_str(&e.to_string());
        }
        out.push('\n');
    }
    if stdout {
        let _ = std::io::stdout().lock().write_all(out.as_bytes());
    } else {
        let _ = std::io::stderr().lock().write_all(out.as_bytes());
    }
}

/// The errors of a whole program whose root file has text `src`.
fn check_errors(root: &Path, src: &str) -> Vec<Error> {
    match module::load_with(root, Some(src.to_string())) {
        Err(e) => e,
        Ok(mut m) => {
            let r = check::check(&mut m, true).err().unwrap_or_default();
            std::mem::forget(m);
            r
        }
    }
}

/// Root path for module resolution (`--stdin` resolves imports from the
/// current directory).
/// A variable name for `--pass-env` / `--allow env=`: the value always comes
/// from the caller's environment, so `NAME=VALUE` (which would inject e.g.
/// `LD_PRELOAD` into the sandbox) and empty names are usage errors.
fn env_name(flag: &str, n: String) -> String {
    if n.is_empty() || n.contains('=') || n.contains('\0') {
        usage(&format!(
            "{}: '{}' is not a variable NAME (values come from your environment)",
            flag, n
        ));
    }
    n
}

fn root_path(a: &Args) -> PathBuf {
    if a.stdin {
        std::env::current_dir()
            .unwrap_or_default()
            .join("stdin.lim")
    } else {
        PathBuf::from(&a.file)
    }
}

/// `check --stdin` / `--partial` / `--expect`. With `--stdin` all output goes
/// to stdout. Returns the exit code.
fn check_text(a: &Args) -> i32 {
    use std::io::Read;
    let mut bytes = Vec::new();
    let read = if a.stdin {
        std::io::stdin().read_to_end(&mut bytes)
    } else {
        std::fs::File::open(&a.file).and_then(|mut f| f.read_to_end(&mut bytes))
    };
    if let Err(e) = read {
        eprintln!("limits: cannot read '{}': {}", a.file, e);
        return 2;
    }
    let src = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            // Reported like any other diagnostic (JSON on stdout for --stdin).
            let (err, ok) = module::utf8_error(&e);
            emit_errors(vec![err], &a.file, Some(ok), a.json, a.stdin);
            return 1;
        }
    };
    let root = root_path(a);
    let t = Instant::now();
    let errs = if a.partial {
        assist::partial(&root, &src, true)
    } else {
        check_errors(&root, &src)
    };
    let t_check = t.elapsed();
    let failed = !errs.is_empty();
    emit_errors(errs, &a.file, Some(&src), a.json, a.stdin);
    let t = Instant::now();
    if a.expect {
        let x = assist::expect(&root, &src, true);
        if a.stdin {
            println!("{}", x);
        } else {
            eprintln!("{}", x);
        }
    }
    if a.time {
        eprintln!(
            "check: {:.3} ms, expect: {:.3} ms",
            t_check.as_secs_f64() * 1e3,
            t.elapsed().as_secs_f64() * 1e3
        );
    }
    i32::from(failed)
}

/// `limits fix file.lim`: apply every machine-applicable fix in place, re-check
/// (fixes can uncover more), and report what changed. Returns the exit code.
fn fix_file(a: &Args) -> i32 {
    let path = PathBuf::from(&a.file);
    let mut total = 0;
    let mut errs = Vec::new();
    for _ in 0..20 {
        let Ok(src) = std::fs::read_to_string(&path) else {
            eprintln!("limits: cannot read '{}'", a.file);
            return 2;
        };
        errs = check_errors(&path, &src);
        // Machine fixes in the root file, first come first served; a fix
        // overlapping an earlier one waits for the next round.
        let mut taken: Vec<(usize, usize)> = Vec::new();
        let mut edits = Vec::new();
        let mut done = Vec::new();
        for e in errs.iter_mut().filter(|e| e.file.is_empty()) {
            diag::enrich(e, Some(&src));
            for f in e.more.fixes.iter().filter(|f| f.machine) {
                let spans: Vec<(usize, usize)> = f
                    .edits
                    .iter()
                    .map(|d| {
                        let x = diag::offset(&src, d.line, d.col);
                        (x, diag::offset(&src, d.end_line, d.end_col).max(x))
                    })
                    .collect();
                let dup = f.edits.iter().all(|d| edits.contains(d));
                let clash = spans.iter().any(|s| {
                    taken
                        .iter()
                        .any(|t| s.0 < t.1.max(t.0 + 1) && t.0 < s.1.max(s.0 + 1))
                });
                if dup || clash {
                    continue;
                }
                taken.extend(spans);
                edits.extend(f.edits.iter().cloned());
                done.push(format!("{}:{}:{}: {}", a.file, e.line, e.col, f.msg));
            }
        }
        if edits.is_empty() {
            break;
        }
        let (new, _) = diag::apply(&src, &edits);
        if let Err(e) = std::fs::write(&path, new) {
            eprintln!("limits: cannot write '{}': {}", a.file, e);
            return 2;
        }
        for d in &done {
            println!("fixed {}", d);
        }
        total += done.len();
    }
    let n = errs.len();
    let src = std::fs::read_to_string(&path).ok();
    emit_errors(errs, &a.file, src.as_deref(), a.json, false);
    println!("{} fix(es) applied, {} error(s) remain", total, n);
    i32::from(n > 0)
}

/// `--show-perms`: the capabilities the program declares (the root file's
/// `uses` line, which bounds every module's) and the runtime grants.
fn show_perms(a: &Args, mods: &[module::Module]) {
    let mut caps: Vec<&str> = Vec::new();
    for (u, _, _) in &mods[0].prog.uses {
        if !caps.contains(&u.as_str()) {
            caps.push(u);
        }
    }
    caps.sort_by_key(|c| check::CAPS.iter().position(|x| x == c));
    let list = |xs: &[String], none: &str| {
        if xs.is_empty() {
            none.to_string()
        } else {
            xs.join(", ")
        }
    };
    println!("program: {}", a.file);
    if caps.is_empty() {
        println!("uses: (none: compute and print only)");
    } else {
        println!("uses: {}", caps.join(", "));
    }
    let cwd = "current directory at run time";
    for c in &caps {
        match *c {
            "fs.read" => println!("  fs.read: {}", list(&a.opts.allow_read, cwd)),
            "fs.write" => println!("  fs.write: {}", list(&a.opts.allow_write, cwd)),
            "env" => println!("  env: {}", list(&a.opts.allow_env, "all variables")),
            _ => {}
        }
    }
    for m in &mods[1..] {
        let us: Vec<&str> = m.prog.uses.iter().map(|u| u.0.as_str()).collect();
        if !us.is_empty() {
            println!("module {}: uses {}", m.file, us.join(", "));
        }
    }
}

/// Compile the C translation units (in parallel when there are several) and
/// link them into `out`.
fn cc(tus: &[String], out: &Path, debug: bool, comp: &Compiler) {
    let (compiler, tcc) = match comp {
        Compiler::Tcc(p) => (p.to_string_lossy().into_owned(), true),
        Compiler::Gcc(c) => (c.clone(), false),
    };
    let opt: &[&str] = match (tcc, debug) {
        // tcc does not optimise; -g would only slow it down.
        (true, _) => &[],
        (false, true) => &["-O0", "-g"],
        (false, false) => &["-O2", "-DNDEBUG"],
    };
    let cmd = || {
        let mut c = Command::new(&compiler);
        if tcc {
            // Its diagnostics are dropped: on failure the system compiler
            // rebuilds the program and reports (see below).
            if std::env::var_os("LIMITS_TCC_STRICT").is_none() {
                c.stderr(std::process::Stdio::null());
            }
            c.args(["-Wall"]);
        } else {
            c.args([
                "-std=gnu11",
                "-fwrapv",
                "-Wall",
                "-Wextra",
                "-Wno-infinite-recursion",
            ]);
        }
        c.args(opt);
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
    if !ok && tcc && std::env::var_os("LIMITS_TCC_STRICT").is_none() {
        // tcc has fixed internal limits (e.g. very deeply nested
        // expressions); fall back to the system compiler, unoptimised.
        let sys = std::env::var("CC").unwrap_or_else(|_| "cc".into());
        return cc(tus, out, true, &Compiler::Gcc(sys));
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
    let special = (a.cmd == "check" && (a.stdin || a.partial)) || a.cmd == "fix";
    if special {
        let code = std::thread::scope(|s| {
            std::thread::Builder::new()
                .stack_size(256 << 20)
                .spawn_scoped(s, || {
                    if a.cmd == "fix" {
                        fix_file(&a)
                    } else {
                        check_text(&a)
                    }
                })
                .expect("spawn frontend thread")
                .join()
                .unwrap_or(101)
        });
        exit(code);
    }
    let t0 = Instant::now();
    // The frontend recurses over the syntax tree; the parser caps nesting and
    // a large (lazily committed) stack makes that cap safe in every pass.
    let (mut c_src, tests, caps) = std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn_scoped(s, || frontend(&a))
            .expect("spawn frontend thread")
            .join()
            .unwrap_or_else(|_| exit(101))
    });
    let t_front = t0.elapsed();
    // Test-only escape hatch (debug builds of limits only): compile without
    // the runtime path check, to show the OS sandbox still denies access.
    if cfg!(debug_assertions)
        && std::env::var_os("LIMITS_UNSAFE_NO_RUNTIME_FS").is_some_and(|v| v == "1")
    {
        for tu in &mut c_src {
            tu.insert_str(0, "#define LM_UNSAFE_NO_RUNTIME_FS 1\n");
        }
    }
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
    // Find the sandbox before compiling, so a missing one fails fast.
    let sandbox = if a.cmd == "build" { None } else { launcher(&a) };
    let t1 = Instant::now();
    cc(&c_src, &out, a.debug, &pick_compiler(&a));
    if a.time {
        eprintln!(
            "frontend: {:.2} ms, cc: {:.2} ms",
            t_front.as_secs_f64() * 1e3,
            t1.elapsed().as_secs_f64() * 1e3
        );
    }
    if a.cmd == "test" {
        let mk = |i: usize| {
            let mut c = sandboxed(&a, sandbox.as_deref(), &caps, &out);
            c.arg(i.to_string());
            c
        };
        run_tests(&mk, &tests, a.json, a.timeout);
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return;
    }
    if a.cmd == "run" {
        let mut c = sandboxed(&a, sandbox.as_deref(), &caps, &out);
        if caps.iter().any(|c| c == "args") {
            c.args(&a.prog_args);
        }
        let limit = std::time::Duration::from_secs(a.timeout);
        let status = c.spawn().map(|mut ch| wait_limited(&mut ch, limit));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        match status {
            Ok(Some(s)) => exit(exit_code(s, a.json)),
            Ok(None) => {
                let m = format!("program timed out (wall limit {} s)", a.timeout);
                if a.json {
                    eprintln!("{}", diag::json("runtime", "", 0, 0, &m, ""));
                } else {
                    eprintln!("limits: {}", m);
                }
                exit(128 + 9)
            }
            Err(e) => {
                eprintln!("limits: cannot run program: {}", e);
                exit(2)
            }
        }
    }
}

/// The limits-sandbox launcher: next to the limits executable, else
/// `$LIMITS_SANDBOX`, else on `PATH`, else `sandbox/target/release` in a
/// directory above the limits executable (the repository layout). `None`
/// with `--no-sandbox` (after a warning); exits with an error if missing.
fn launcher(a: &Args) -> Option<PathBuf> {
    if a.no_sandbox {
        eprintln!(
            "limits: warning: --no-sandbox: running without the OS sandbox (only the runtime checks and the --timeout wall limit apply)"
        );
        return None;
    }
    const NAME: &str = "limits-sandbox";
    let exe = std::env::current_exe().ok();
    let next_to = exe.as_ref().and_then(|e| e.parent().map(|d| d.join(NAME)));
    let env = std::env::var_os("LIMITS_SANDBOX")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    let on_path = std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join(NAME))
            .find(|p| p.is_file())
    });
    let repo = exe.as_ref().and_then(|e| {
        e.ancestors()
            .map(|d| d.join("sandbox/target/release").join(NAME))
            .find(|p| p.is_file())
    });
    let found = [next_to, env, on_path, repo]
        .into_iter()
        .flatten()
        .find(|p| p.is_file());
    if found.is_none() {
        eprintln!(
            "limits: cannot find {NAME} (looked next to limits, in $LIMITS_SANDBOX and on PATH)\n  \
             hint: build it with 'cargo build --release' in the sandbox/ directory and put it next to \
             limits or set LIMITS_SANDBOX, or pass --no-sandbox to run without the OS sandbox"
        );
        exit(2);
    }
    found
}

/// Memory the sandbox allows beyond `--max-mem`: code, stacks, the C library
/// and allocator overhead count against the address-space limit too.
const MEM_HEADROOM_MB: u64 = 512;

/// A command that runs `prog` under limits-sandbox (or directly, without
/// one), with OS grants that mirror the program's declared capabilities.
fn sandboxed(a: &Args, sandbox: Option<&Path>, caps: &[String], prog: &Path) -> Command {
    let has = |c: &str| caps.iter().any(|x| x == c);
    let mut cmd = match sandbox {
        None => Command::new(prog),
        Some(sb) => {
            let mut c = Command::new(sb);
            c.arg("--quiet");
            let cwd = || {
                std::env::current_dir()
                    .and_then(std::fs::canonicalize)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|e| {
                        eprintln!("limits: cannot resolve the current directory: {}", e);
                        exit(2)
                    })
            };
            // Same defaults as the runtime: no --allow means the current
            // directory. fs.write also allows reads.
            let dirs = |xs: &[String]| {
                if xs.is_empty() {
                    vec![cwd()]
                } else {
                    xs.to_vec()
                }
            };
            if has("fs.read") || has("fs.write") {
                for d in dirs(&a.opts.allow_read) {
                    c.arg("--read").arg(d);
                }
            }
            if has("fs.write") {
                for d in dirs(&a.opts.allow_write) {
                    c.arg("--write").arg(d);
                }
            }
            if has("env") {
                for n in a.pass_env.iter().chain(&a.opts.allow_env) {
                    c.arg("--env").arg(n);
                }
            }
            let mem = (a.opts.max_mem >> 20).saturating_add(MEM_HEADROOM_MB);
            c.args(["--net", "none"])
                .arg("--mem-mb")
                .arg(mem.to_string())
                .arg("--fsize-mb")
                .arg(mem.to_string())
                .arg("--cpu-sec")
                .arg(a.timeout.to_string())
                // A backstop: limits enforces the wall limit itself (also
                // without the sandbox) and reports it, so it must fire first.
                .arg("--wall-sec")
                .arg(a.timeout.saturating_add(1).to_string())
                .arg("--")
                .arg(prog);
            c
        }
    };
    if !has("stdin") {
        cmd.stdin(std::process::Stdio::null());
    }
    cmd
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

/// Wait for `child` at most `limit`. On expiry kill it and return `None`.
/// Killing the limits-sandbox launcher also kills the sandboxed program (the
/// launcher sets PR_SET_PDEATHSIG), so nothing outlives limits; without the
/// sandbox the child is the program itself.
fn wait_limited(
    child: &mut std::process::Child,
    limit: std::time::Duration,
) -> Option<std::process::ExitStatus> {
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return Some(s),
            Ok(None) if t0.elapsed() < limit => {
                std::thread::sleep(std::time::Duration::from_millis(5))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Wall-clock limit per test process, so a test built with `--unchecked`
/// (no step budget) that never ends cannot hang the whole run.
const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Run every test in its own process (`prog <index>`), so a test that fails,
/// aborts, exhausts a budget or times out cannot affect the others.
fn run_tests(mk: &dyn Fn(usize) -> Command, names: &[String], json: bool, timeout: u64) {
    let limit = TEST_TIMEOUT.min(std::time::Duration::from_secs(timeout));
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
        let child = mk(i).stdout(Stdio::null()).stderr(Stdio::piped()).spawn();
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
        let status = wait_limited(&mut child, limit);
        let err = reader.join().unwrap_or_default();
        let why = match status {
            Some(s) if s.success() => {
                say(format!("ok {}", shown));
                continue;
            }
            None => format!("timed out after {} s", limit.as_secs()),
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
