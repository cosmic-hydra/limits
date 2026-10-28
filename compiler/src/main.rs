use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use limits::error::LimError;
use limits::interp::{Interpreter, Limits, BUILTINS};
use limits::protocol::{self, quote};
use limits::sandbox::SandboxPolicy;
use limits::{codegen, lexer, parser, resolve, semantic, Engine};

const USAGE: &str = "usage: lim run|check <file.lim|file.lim.age|-> [--json]
       lim run <source> [--engine vm|tree] [--max-steps N] [--max-depth N]
           [--sandbox unrestricted|restricted|pure] [--allow-builtin NAME] [--deny-builtin NAME]
           [--max-output-bytes N] [--timeout-ms N]
       lim index <source> [--json]
       lim symbols <source> [--json] [--limit N]
       lim definition|references|complete <source> --line N --col N [--prefix TEXT] [--limit N]
       lim capabilities [--json]
       lim seal <file.lim|-> --recipient age1... --out <file.lim.age> [--json]
       lim unseal <file.lim.age> --identity <key-file> --out <file.lim> [--json]
       encrypted sources require --identity <key-file>; '-' reads bounded UTF-8 stdin";

struct Options {
    command: String,
    source: Option<String>,
    json: bool,
    engine: Engine,
    limits: Limits,
    policy: SandboxPolicy,
    line: Option<usize>,
    col: Option<usize>,
    prefix: String,
    limit: usize,
    identity: Option<PathBuf>,
    recipient: Option<String>,
    output: Option<PathBuf>,
}
fn option_allowed(command: &str, flag: &str) -> bool {
    match flag {
        "--json" => true,
        "--engine" | "--max-steps" | "--max-depth" | "--sandbox" | "--allow-builtin"
        | "--deny-builtin" | "--max-output-bytes" | "--timeout-ms" => command == "run",
        "--identity" => !matches!(command, "seal" | "capabilities"),
        "--recipient" => command == "seal",
        "--out" => matches!(command, "seal" | "unseal"),
        "--line" | "--col" => matches!(command, "definition" | "references" | "complete"),
        "--prefix" => command == "complete",
        "--limit" => matches!(
            command,
            "symbols" | "definition" | "references" | "complete"
        ),
        _ => false,
    }
}
fn parse_options(args: &[String]) -> Result<Options, String> {
    let command = args.first().ok_or(USAGE)?.as_str();
    if !matches!(
        command,
        "run"
            | "check"
            | "index"
            | "symbols"
            | "definition"
            | "references"
            | "complete"
            | "capabilities"
            | "seal"
            | "unseal"
    ) {
        return Err(format!("unknown command '{command}'\n{USAGE}"));
    }
    let json = args.iter().any(|s| s == "--json")
        || !matches!(command, "run" | "check" | "seal" | "unseal");
    let mut options = Options {
        command: command.into(),
        source: None,
        json,
        engine: Engine::Tree,
        limits: Limits::default(),
        policy: if json {
            SandboxPolicy::restricted()
        } else {
            SandboxPolicy::default()
        },
        line: None,
        col: None,
        prefix: String::new(),
        limit: 100,
        identity: None,
        recipient: None,
        output: None,
    };
    let mut allow = Vec::new();
    let mut deny = Vec::new();
    let mut explicit_output = None;
    let mut explicit_timeout = None;
    let mut seen = BTreeSet::new();
    let mut args = args[1..].iter();
    while let Some(flag) = args.next() {
        if !flag.starts_with("--") {
            if command == "capabilities" || options.source.replace(flag.clone()).is_some() {
                return Err("expected exactly one source path".into());
            }
            continue;
        }
        if !option_allowed(command, flag) {
            return Err(format!("unsupported flag '{flag}' for {command}"));
        }
        if !matches!(flag.as_str(), "--allow-builtin" | "--deny-builtin")
            && !seen.insert(flag.clone())
        {
            return Err(format!("duplicate flag '{flag}'"));
        }
        if flag == "--json" {
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for '{flag}'"))?;
        let number = || {
            value
                .parse::<u64>()
                .map_err(|_| format!("'{flag}' requires a nonnegative integer"))
        };
        match flag.as_str() {
            "--engine" => {
                options.engine = match value.as_str() {
                    "tree" => Engine::Tree,
                    "vm" => Engine::Vm,
                    _ => return Err("expected --engine vm|tree".into()),
                }
            }
            "--max-steps" => options.limits.max_steps = number()?,
            "--max-depth" => {
                options.limits.max_depth =
                    usize::try_from(number()?).map_err(|_| "depth exceeds host capacity")?
            }
            "--sandbox" => {
                options.policy = match value.as_str() {
                    "unrestricted" => SandboxPolicy::default(),
                    "restricted" => SandboxPolicy::restricted(),
                    "pure" => SandboxPolicy::pure(),
                    _ => return Err("expected --sandbox unrestricted|restricted|pure".into()),
                }
            }
            "--allow-builtin" => allow.push(value.clone()),
            "--deny-builtin" => deny.push(value.clone()),
            "--max-output-bytes" => explicit_output = Some(number()?),
            "--timeout-ms" => explicit_timeout = Some(Duration::from_millis(number()?)),
            "--line" | "--col" => {
                let n = usize::try_from(number()?).map_err(|_| "position exceeds host capacity")?;
                if n == 0 {
                    return Err("positions are 1-based".into());
                }
                if flag == "--line" {
                    options.line = Some(n);
                } else {
                    options.col = Some(n);
                }
            }
            "--prefix" => options.prefix = value.clone(),
            "--limit" => {
                options.limit =
                    usize::try_from(number()?).map_err(|_| "limit exceeds host capacity")?;
                if options.limit == 0 || options.limit > 10_000 {
                    return Err("--limit must be between 1 and 10000".into());
                }
            }
            "--identity" => options.identity = Some(value.into()),
            "--recipient" => options.recipient = Some(value.clone()),
            "--out" => options.output = Some(value.into()),
            _ => unreachable!(),
        }
    }
    if command != "capabilities" {
        let path = options.source.as_deref().ok_or("missing source path")?;
        if path != "-" && !path.ends_with(".lim") && !path.ends_with(".lim.age") {
            return Err("expected a .lim or .lim.age source file".into());
        }
        if path.ends_with(".lim.age") && options.identity.is_none() {
            return Err("encrypted source requires --identity <key-file>".into());
        }
        if !path.ends_with(".lim.age") && options.identity.is_some() {
            return Err("--identity requires a .lim.age source".into());
        }
        if command == "seal" && path.ends_with(".lim.age") {
            return Err("source is already sealed".into());
        }
        if command == "unseal" && !path.ends_with(".lim.age") {
            return Err("unseal requires a .lim.age source".into());
        }
    }
    if matches!(command, "definition" | "references" | "complete")
        && (options.line.is_none() || options.col.is_none())
    {
        return Err("query requires --line N --col N".into());
    }
    if matches!(command, "seal" | "unseal") && options.output.is_none() {
        return Err("encryption commands require --out <new-file>".into());
    }
    if command == "seal" && options.recipient.is_none() {
        return Err("seal requires --recipient <age-public-key>".into());
    }
    for name in allow.iter().chain(&deny) {
        if !BUILTINS.contains(&name.as_str()) {
            return Err(format!("unknown builtin '{name}'"));
        }
    }
    if !allow.is_empty() {
        options.policy.allowed_builtins = Some(allow.into_iter().collect());
    }
    if !deny.is_empty() {
        let allowed = options
            .policy
            .allowed_builtins
            .get_or_insert_with(|| BUILTINS.iter().map(|s| s.to_string()).collect());
        for name in deny {
            allowed.remove(&name);
        }
    }
    if let Some(n) = explicit_output {
        options.policy.max_output_bytes = Some(n);
    } else if json && options.policy.max_output_bytes.is_none() {
        options.policy.max_output_bytes = Some(1024 * 1024);
    }
    if let Some(n) = explicit_timeout {
        options.policy.timeout = Some(n);
    }
    options.policy.validate()?;
    Ok(options)
}
fn bounded_read(path: &str, max: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    if path == "-" {
        std::io::stdin()
            .lock()
            .take(max as u64 + 1)
            .read_to_end(&mut bytes)
    } else {
        std::fs::File::open(path)
            .map_err(|_| "cannot open source file")?
            .take(max as u64 + 1)
            .read_to_end(&mut bytes)
    }
    .map_err(|_| "cannot read source")?;
    if bytes.len() > max {
        return Err(format!("source size limit exceeded ({max} bytes)"));
    }
    Ok(bytes)
}
fn read_source(options: &Options) -> Result<String, String> {
    let path = options.source.as_deref().unwrap();
    if path.ends_with(".lim.age") {
        let bytes = bounded_read(path, limits::sealed::MAX_CIPHERTEXT_BYTES)?;
        limits::sealed::Age::default()
            .open(&bytes, options.identity.as_deref().unwrap())
            .map_err(|e| e.to_string())
    } else {
        let bytes = bounded_read(path, lexer::MAX_SOURCE_BYTES)?;
        String::from_utf8(bytes).map_err(|_| "source is not valid UTF-8".into())
    }
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "cannot create output (existing files are never overwritten)")?;
    if file.write_all(bytes).and_then(|_| file.sync_all()).is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err("cannot write output".into());
    }
    Ok(())
}
fn range_json(range: &semantic::SourceRange) -> String {
    format!(
        "{{\"start\":{{\"line\":{},\"col\":{}}},\"end\":{{\"line\":{},\"col\":{}}}}}",
        range.start.line, range.start.col, range.end.line, range.end.col
    )
}
fn symbol_json(symbol: &semantic::Symbol) -> String {
    format!("{{\"id\":{},\"name\":{},\"kind\":{},\"scope\":{},\"declarations\":[{}],\"parameters\":[{}],\"captures\":[{}]}}",symbol.id,quote(&symbol.name),quote(&symbol.kind),symbol.scope,symbol.declarations.iter().map(range_json).collect::<Vec<_>>().join(","),symbol.parameters.iter().map(|s|quote(s)).collect::<Vec<_>>().join(","),symbol.captures.iter().map(usize::to_string).collect::<Vec<_>>().join(","))
}
fn reference_json(reference: &semantic::Reference) -> String {
    format!(
        "{{\"name\":{},\"range\":{},\"scope\":{},\"role\":{},\"binding\":{},\"candidates\":[{}]}}",
        quote(&reference.name),
        range_json(&reference.range),
        reference.scope,
        quote(&reference.role),
        reference
            .binding
            .map_or_else(|| "null".into(), |n| n.to_string()),
        reference
            .candidates
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}
fn query(options: &Options, source: &str) -> Result<(String, Vec<LimError>), LimError> {
    let index = semantic::analyze(source)?;
    let data = match options.command.as_str() {
        "index" => index.to_json(),
        "symbols" => {
            let symbols: Vec<_> = index
                .symbols
                .iter()
                .filter(|s| s.kind != "builtin")
                .collect();
            format!(
                "{{\"symbols\":[{}],\"truncated\":{}}}",
                symbols
                    .iter()
                    .take(options.limit)
                    .map(|s| symbol_json(s))
                    .collect::<Vec<_>>()
                    .join(","),
                symbols.len() > options.limit
            )
        }
        "definition" => {
            let definitions = index.definitions_at(options.line.unwrap(), options.col.unwrap());
            format!(
                "{{\"definitions\":[{}],\"resolution\":\"lexical-candidates\",\"truncated\":{}}}",
                definitions
                    .iter()
                    .take(options.limit)
                    .map(|s| symbol_json(s))
                    .collect::<Vec<_>>()
                    .join(","),
                definitions.len() > options.limit
            )
        }
        "references" => {
            let ids: BTreeSet<_> = index
                .definitions_at(options.line.unwrap(), options.col.unwrap())
                .iter()
                .map(|s| s.id)
                .collect();
            let references: Vec<_> = index
                .references
                .iter()
                .filter(|r| r.candidates.iter().any(|id| ids.contains(id)))
                .collect();
            format!(
                "{{\"references\":[{}],\"truncated\":{}}}",
                references
                    .iter()
                    .take(options.limit)
                    .map(|r| reference_json(r))
                    .collect::<Vec<_>>()
                    .join(","),
                references.len() > options.limit
            )
        }
        "complete" => {
            let symbols =
                index.completions(options.line.unwrap(), options.col.unwrap(), &options.prefix);
            format!(
                "{{\"completions\":[{}],\"truncated\":{}}}",
                symbols
                    .iter()
                    .take(options.limit)
                    .map(|s| symbol_json(s))
                    .collect::<Vec<_>>()
                    .join(","),
                symbols.len() > options.limit
            )
        }
        _ => unreachable!(),
    };
    Ok((data, index.diagnostics))
}
fn write_stdout(value: &str) -> Result<(), String> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "{value}")
        .and_then(|_| out.flush())
        .map_err(|_| "cannot write response".into())
}
fn write_stderr(value: &str) {
    // A failed diagnostic sink must not turn a program/host error into a panic.
    let _ = writeln!(std::io::stderr().lock(), "{value}");
}
fn generic_error(command: &str, source: Option<&str>, message: &str, kind: &str, json: bool) -> u8 {
    if json {
        let mut end = message.len().min(protocol::MAX_DIAGNOSTIC_MESSAGE_BYTES);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        let run_fields = if command == "run" {
            ",\"stdout\":\"\",\"engine\":null"
        } else {
            ""
        };
        let encoded=format!("{{\"schema_version\":1,\"command\":{},\"ok\":false,\"source\":{},\"data\":null,\"diagnostic_count\":1,\"diagnostics_truncated\":false,\"diagnostics\":[{{\"code\":{},\"kind\":{},\"severity\":\"error\",\"message\":{},\"line\":null,\"col\":null,\"hint\":\"Inspect invocation and host configuration.\",\"message_truncated\":{},\"original_message_bytes\":{}}}]{run_fields}}}",quote(command),source.map_or_else(||"null".into(),quote),quote(if kind=="usage"{"LIM_USAGE"}else{"LIM_HOST"}),quote(kind),quote(&message[..end]),end<message.len(),message.len());
        if write_stdout(&encoded).is_err() {
            write_stderr("error: cannot write response");
        }
    } else {
        write_stderr(&format!("error: {message}"));
    }
    2
}
fn program_errors(options: &Options, errors: Vec<LimError>) -> u8 {
    if options.json {
        let response = if options.command == "run" {
            protocol::run_errors_response(options.source.as_deref(), options.engine, "", &errors)
        } else if options.command == "check" {
            protocol::check_response(options.source.as_deref(), &errors)
        } else {
            protocol::command_response(&options.command, options.source.as_deref(), "null", &errors)
        };
        if write_stdout(&response).is_err() {
            return 2;
        }
    } else {
        for e in errors {
            write_stderr(&format!(
                "{}: {e}",
                options.source.as_deref().unwrap_or("-")
            ));
        }
    }
    1
}
fn execute(options: Options) -> u8 {
    if options.command == "capabilities" {
        return if write_stdout(&protocol::capabilities_json()).is_ok() {
            0
        } else {
            2
        };
    }
    let source = match read_source(&options) {
        Ok(s) => s,
        Err(e) => {
            if e.starts_with("source size limit exceeded")
                && !options.source.as_ref().unwrap().ends_with(".lim.age")
            {
                return program_errors(
                    &options,
                    vec![LimError::new(
                        limits::error::Kind::Lex,
                        e,
                        limits::error::Span { line: 1, col: 1 },
                    )],
                );
            }
            return generic_error(
                &options.command,
                options.source.as_deref(),
                &e,
                "host",
                options.json,
            );
        }
    };
    if matches!(options.command.as_str(), "seal" | "unseal") {
        let bytes = if options.command == "seal" {
            match limits::sealed::Age::default()
                .seal(&source, options.recipient.as_deref().unwrap())
            {
                Ok(b) => b,
                Err(e) => {
                    return generic_error(
                        &options.command,
                        options.source.as_deref(),
                        &e.to_string(),
                        "host",
                        options.json,
                    )
                }
            }
        } else {
            source.into_bytes()
        };
        if let Err(e) = write_new(options.output.as_deref().unwrap(), &bytes) {
            return generic_error(
                &options.command,
                options.source.as_deref(),
                &e,
                "host",
                options.json,
            );
        }
        if options.json {
            let data = format!(
                "{{\"output\":{},\"bytes\":{},\"format\":{}}}",
                quote(&options.output.unwrap().to_string_lossy()),
                bytes.len(),
                quote(if options.command == "seal" {
                    "age-v1"
                } else {
                    "utf-8-limits"
                })
            );
            if write_stdout(&protocol::command_response(
                &options.command,
                options.source.as_deref(),
                &data,
                &[],
            ))
            .is_err()
            {
                return 2;
            }
        }
        return 0;
    }
    if matches!(
        options.command.as_str(),
        "index" | "symbols" | "definition" | "references" | "complete"
    ) {
        return match query(&options, &source) {
            Ok((data, errors)) => {
                let exit = u8::from(!errors.is_empty());
                if write_stdout(&protocol::command_response(
                    &options.command,
                    options.source.as_deref(),
                    &data,
                    &errors,
                ))
                .is_err()
                {
                    2
                } else {
                    exit
                }
            }
            Err(e) => program_errors(&options, vec![e]),
        };
    }
    let program = match lexer::lex(&source).and_then(parser::parse) {
        Ok(p) => p,
        Err(e) => return program_errors(&options, vec![e]),
    };
    let errors = resolve::check(&program);
    if !errors.is_empty() {
        if options.command == "run" && options.json {
            if write_stdout(&protocol::run_errors_response(
                options.source.as_deref(),
                options.engine,
                "",
                &errors,
            ))
            .is_err()
            {
                return 2;
            }
            return 1;
        }
        return program_errors(&options, errors);
    }
    if options.command == "check" {
        return if options.json
            && write_stdout(&protocol::check_response(options.source.as_deref(), &[])).is_err()
        {
            2
        } else {
            0
        };
    }
    let run = |out: &mut dyn Write| match options.engine {
        Engine::Tree => {
            Interpreter::with_policy(out, options.limits, options.policy.clone()).run(&program)
        }
        Engine::Vm => codegen::compile(&program).and_then(|compiled| {
            limits::vm::Vm::with_policy(out, options.limits, options.policy.clone()).run(compiled)
        }),
    };
    if options.json {
        let mut out = Vec::new();
        let result = run(&mut out);
        let exit = u8::from(result.is_err());
        if write_stdout(&protocol::run_response(
            options.source.as_deref(),
            options.engine,
            &String::from_utf8_lossy(&out),
            &result,
        ))
        .is_err()
        {
            2
        } else {
            exit
        }
    } else {
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        match run(&mut out).and_then(|_| {
            out.flush().map_err(|e| {
                LimError::new(
                    limits::error::Kind::Runtime,
                    e.to_string(),
                    limits::error::Span { line: 1, col: 1 },
                )
            })
        }) {
            Ok(()) => 0,
            Err(e) => {
                write_stderr(&format!("{}: {e}", options.source.unwrap()));
                1
            }
        }
    }
}
fn main() -> ExitCode {
    // OS arguments may contain invalid Unicode. Reject them through the CLI
    // error contract instead of letting std::env::args panic before dispatch.
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let json = args.iter().any(|a| a == "--json")
        || args.first().is_some_and(|s| {
            matches!(
                s.to_str(),
                Some(
                    "capabilities" | "index" | "symbols" | "definition" | "references" | "complete"
                )
            )
        });
    if args.iter().any(|arg| arg.to_str().is_none()) {
        return ExitCode::from(generic_error(
            args.first().and_then(|s| s.to_str()).unwrap_or(""),
            None,
            "arguments must be valid UTF-8",
            "usage",
            json,
        ));
    }
    let args: Vec<String> = args
        .into_iter()
        .map(|arg| arg.into_string().expect("validated UTF-8 argument"))
        .collect();
    if args.first().is_some_and(|a| a == "--help" || a == "-h") {
        return ExitCode::from(if write_stdout(USAGE).is_ok() { 0 } else { 2 });
    }
    match parse_options(&args) {
        Ok(options) => ExitCode::from(execute(options)),
        Err(e) => ExitCode::from(generic_error(
            args.first().map_or("", String::as_str),
            None,
            &e,
            "usage",
            json,
        )),
    }
}
