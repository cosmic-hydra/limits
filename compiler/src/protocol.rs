//! Compact JSON protocol for agents and editor integrations.
//!
//! The wire schema is versioned independently of the language. Diagnostic codes
//! encode the trusted `LimError::kind`; user-controlled assertion messages must
//! not be promoted into authoritative error categories. Hints are advisory text,
//! never executable fixes or permission requests.

use std::fmt::Write as _;

use crate::error::{Kind, LimError};
use crate::interp::Limits;
use crate::Engine;

pub const SCHEMA_VERSION: u32 = 1;
/// Maximum unescaped UTF-8 message bytes serialized for one diagnostic.
/// The internal error remains intact; truncation is explicit on the wire.
pub const MAX_DIAGNOSTIC_MESSAGE_BYTES: usize = 16 * 1024;
/// Maximum number of diagnostics serialized in one response. Envelopes expose
/// total count and truncation so a client can refine its request or retry.
pub const MAX_DIAGNOSTICS: usize = 1024;

fn bounded_message(message: &str) -> (&str, bool) {
    if message.len() <= MAX_DIAGNOSTIC_MESSAGE_BYTES {
        return (message, false);
    }
    let mut end = MAX_DIAGNOSTIC_MESSAGE_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    (&message[..end], true)
}

/// Encode a Rust UTF-8 string as one JSON string, including surrounding quotes.
/// All JSON control characters are escaped. Unicode scalar values are preserved;
/// line/paragraph separators are escaped for consumers embedding JSON in JS.
pub fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len().saturating_add(2));
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if c <= '\u{1f}' => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn kind_name(kind: &Kind) -> &'static str {
    match kind {
        Kind::Lex => "lex",
        Kind::Parse => "parse",
        Kind::Check => "check",
        Kind::Runtime => "runtime",
    }
}

/// Stable machine code based on trusted error kind, not untrusted message text.
pub fn diagnostic_code(error: &LimError) -> &'static str {
    match error.kind {
        Kind::Lex => "LIM_LEX",
        Kind::Parse => "LIM_PARSE",
        Kind::Check => "LIM_CHECK",
        Kind::Runtime => "LIM_RUNTIME",
    }
}

/// Human/agent guidance, not an automatic edit or a reliable security signal.
pub fn recovery_hint(error: &LimError) -> &'static str {
    let msg = error.msg.as_str();
    match error.kind {
        Kind::Lex if msg.starts_with("source size limit exceeded") => {
            "Reduce source size before retrying; execution limits do not change the source-size guard."
        }
        Kind::Lex if msg == "unterminated string" => {
            "Close the string with a double quote and escape embedded quotes with a backslash."
        }
        Kind::Lex if msg.starts_with("unknown escape") => {
            "Use a supported string escape: \\n, \\t, \\\\, or \\\"."
        }
        Kind::Lex => "Inspect the token at this location; use valid literals, identifiers, comments and operators.",
        Kind::Parse if msg.contains("nesting limit exceeded") => {
            "Split the expression or nested control flow into smaller functions or intermediate bindings."
        }
        Kind::Parse if msg == "comparisons cannot be chained; use 'and'" => {
            "Write separate comparisons joined by 'and', for example a < b and b < c."
        }
        Kind::Parse => "Inspect the syntax at this location; statements end with ';' and blocks use braces.",
        Kind::Check if msg.starts_with("duplicate parameter") => {
            "Give each parameter in this function a distinct name."
        }
        Kind::Check if msg.contains("outside of a loop") => {
            "Place break/continue inside a loop in the same function; an enclosing function's loop does not count."
        }
        Kind::Check => "Check name spelling and lexical scope; declare values with 'let' or functions with 'fn'.",
        Kind::Runtime if msg.starts_with("step limit exceeded") => {
            "Check loop termination and algorithmic work; increase a host-approved step budget only when intentional."
        }
        Kind::Runtime if msg.starts_with("call depth limit exceeded") => {
            "Check the recursion base case or use iteration; change the host-approved depth budget only when intentional."
        }
        Kind::Runtime if msg.starts_with("sandbox denied builtin") => {
            "Choose an operation permitted by the host policy; the program cannot grant itself a capability."
        }
        Kind::Runtime if msg.starts_with("output limit exceeded") => {
            "Reduce printed output or ask the host to configure an appropriate output budget."
        }
        Kind::Runtime if msg == "execution deadline exceeded" => {
            "Reduce execution work and inspect the host deadline before retrying."
        }
        Kind::Runtime if msg == "execution cancelled" => {
            "The host cancelled this invocation; do not automatically restart it."
        }
        Kind::Runtime if msg.starts_with("undefined variable") => {
            "Check spelling and ensure a visible declaration executes before this access."
        }
        Kind::Runtime if msg.starts_with("assignment to undeclared variable") => {
            "Declare the value with 'let' in the intended scope before assigning to it."
        }
        Kind::Runtime if msg == "division by zero" => "Check the divisor before integer division or remainder.",
        Kind::Runtime if msg == "integer overflow" => {
            "Keep integer arithmetic within signed 64-bit bounds or choose float arithmetic when appropriate."
        }
        Kind::Runtime if msg.contains("out of bounds") => {
            "Check container length and index; negative indices count from the end."
        }
        Kind::Runtime if msg.contains("() takes") => {
            "Check the function signature and supply the expected number of arguments."
        }
        Kind::Runtime if msg.starts_with("value ") && msg.ends_with("limit exceeded") => {
            "Reduce value expansion or comparison work; compact shared graphs can request excessive work in one operation."
        }
        Kind::Runtime => "Inspect the operation and operand types at this location; earlier stdout remains part of the result.",
    }
}

pub fn diagnostic_json(error: &LimError) -> String {
    let (message, truncated) = bounded_message(&error.msg);
    format!(
        "{{\"code\":{},\"kind\":{},\"severity\":\"error\",\"message\":{},\"line\":{},\"col\":{},\"hint\":{},\"message_truncated\":{},\"original_message_bytes\":{}}}",
        quote(diagnostic_code(error)), quote(kind_name(&error.kind)), quote(message),
        error.span.line, error.span.col, quote(recovery_hint(error)), truncated, error.msg.len(),
    )
}

fn diagnostic_array(errors: &[LimError]) -> String {
    let mut out = String::from("[");
    for (i, error) in errors.iter().take(MAX_DIAGNOSTICS).enumerate() {
        if i != 0 {
            out.push(',');
        }
        out.push_str(&diagnostic_json(error));
    }
    out.push(']');
    out
}

fn optional_string(value: Option<&str>) -> String {
    value.map_or_else(|| "null".into(), quote)
}

/// Build a response with caller-generated JSON data. `data_json` must already
/// be valid JSON from a trusted serializer; untrusted strings need `quote`.
pub fn command_response(
    command: &str,
    source: Option<&str>,
    data_json: &str,
    errors: &[LimError],
) -> String {
    format!(
        "{{\"schema_version\":{SCHEMA_VERSION},\"command\":{},\"ok\":{},\"source\":{},\"diagnostic_count\":{},\"diagnostics_truncated\":{},\"diagnostics\":{},\"data\":{}}}",
        quote(command),errors.is_empty(),optional_string(source),errors.len(),errors.len()>MAX_DIAGNOSTICS,diagnostic_array(errors),data_json,
    )
}

pub fn check_response(source: Option<&str>, errors: &[LimError]) -> String {
    command_response("check", source, "null", errors)
}

/// One result envelope, including stdout produced before any runtime failure.
pub fn run_response(
    source: Option<&str>,
    engine: Engine,
    stdout: &str,
    result: &Result<(), LimError>,
) -> String {
    run_errors_response(
        source,
        engine,
        stdout,
        match result {
            Ok(()) => &[],
            Err(error) => std::slice::from_ref(error),
        },
    )
}

/// Use the same run schema for static/front-end failures with multiple errors.
pub fn run_errors_response(
    source: Option<&str>,
    engine: Engine,
    stdout: &str,
    errors: &[LimError],
) -> String {
    let engine = match engine {
        Engine::Tree => "tree",
        Engine::Vm => "vm",
    };
    format!(
        "{{\"schema_version\":{SCHEMA_VERSION},\"command\":\"run\",\"ok\":{},\"source\":{},\"diagnostic_count\":{},\"diagnostics_truncated\":{},\"diagnostics\":{},\"engine\":{},\"stdout\":{}}}",
        errors.is_empty(),optional_string(source),errors.len(),errors.len()>MAX_DIAGNOSTICS,diagnostic_array(errors),quote(engine),quote(stdout),
    )
}

/// Builtin catalog suitable for prompt construction and capability discovery.
/// `pure` means no stdout writes or mutation of arguments; calls may still fail.
pub fn builtin_catalog_json() -> String {
    struct Builtin {
        name: &'static str,
        signature: &'static str,
        min: usize,
        max: Option<usize>,
        pure: bool,
        effects: &'static str,
        notes: &'static str,
    }
    const BUILTINS: &[Builtin] = &[
        Builtin{name:"print",signature:"print(...values) -> nil",min:0,max:None,pure:false,effects:"[\"stdout\"]",notes:"Space-separated values followed by newline; bounded rendering; may fail on writer errors."},
        Builtin{name:"len",signature:"len(string|list|map) -> int",min:1,max:Some(1),pure:true,effects:"[]",notes:"String lengths count Unicode scalar values, not UTF-8 bytes."},
        Builtin{name:"push",signature:"push(list, value) -> nil",min:2,max:Some(2),pure:false,effects:"[\"mutate_argument\"]",notes:"Appends to the shared list; aliases observe the mutation."},
        Builtin{name:"pop",signature:"pop(list) -> value",min:1,max:Some(1),pure:false,effects:"[\"mutate_argument\"]",notes:"Removes the last item; an empty list is a runtime error."},
        Builtin{name:"str",signature:"str(value) -> string",min:1,max:Some(1),pure:true,effects:"[]",notes:"Uses the same bounded, cycle-aware rendering as print."},
        Builtin{name:"int",signature:"int(int|float|string|bool) -> int",min:1,max:Some(1),pure:true,effects:"[]",notes:"Finite floats truncate toward zero using saturating conversion; strings are trimmed and parsed."},
        Builtin{name:"float",signature:"float(int|float|string) -> float",min:1,max:Some(1),pure:true,effects:"[]",notes:"String parsing supports IEEE values; non-finite values may result."},
        Builtin{name:"type",signature:"type(value) -> string",min:1,max:Some(1),pure:true,effects:"[]",notes:"Returns int, float, string, bool, nil, list, map, or fn."},
        Builtin{name:"keys",signature:"keys(map) -> list",min:1,max:Some(1),pure:true,effects:"[]",notes:"Returns a fresh list of string keys in sorted order."},
        Builtin{name:"assert",signature:"assert(condition, message?) -> nil",min:1,max:Some(2),pure:true,effects:"[]",notes:"Fails with the optional rendered message when condition is nil or false."},
        Builtin{name:"get",signature:"get(map, string, default?) -> value",min:2,max:Some(3),pure:true,effects:"[]",notes:"Returns the stored value including nil; returns the eager default or nil only for an absent key."},
        Builtin{name:"has",signature:"has(map, string) -> bool",min:2,max:Some(2),pure:true,effects:"[]",notes:"Tests key presence independently of a stored nil value."},
        Builtin{name:"join",signature:"join(list<string>, string) -> string",min:2,max:Some(2),pure:true,effects:"[]",notes:"Strict string elements and separator; empty list gives an empty string; bounds element count and UTF-8 output bytes before allocation."},
        Builtin{name:"sum",signature:"sum(list<int|float>) -> int|float",min:1,max:Some(1),pure:true,effects:"[]",notes:"Left-to-right checked numeric addition; empty list gives integer zero; mixed numbers preserve ordinary Add semantics; bounded element count."},
    ];
    let mut out = String::from("[");
    for (i, b) in BUILTINS.iter().enumerate() {
        if i != 0 {
            out.push(',');
        }
        write!(out,"{{\"name\":{},\"signature\":{},\"arity_min\":{},\"arity_max\":{},\"pure\":{},\"effects\":{},\"may_fail\":true,\"notes\":{}}}",quote(b.name),quote(b.signature),b.min,b.max.map_or_else(||"null".into(),|n|n.to_string()),b.pure,b.effects,quote(b.notes)).unwrap();
    }
    out.push(']');
    out
}

/// Language/runtime facts; this catalog does not assert that optional external
/// tools (such as an encryption provider) are installed or authenticate agents.
pub fn capabilities_json() -> String {
    let limits = Limits::default();
    format!(
        concat!(
            "{{\"schema_version\":{version},\"language\":\"Limits\",\"language_version\":{language_version},",
            "\"source_extension\":\".lim\",\"engines\":[\"tree\",\"vm\"],\"default_engine\":\"tree\",",
            "\"positions\":{{\"line_base\":1,\"column_base\":1,\"column_unit\":\"unicode_scalar\"}},",
            "\"defaults\":{{\"max_steps\":{steps},\"max_depth\":{depth},\"max_source_bytes\":{source_limit}}},",
            "\"diagnostic_codes\":[\"LIM_LEX\",\"LIM_PARSE\",\"LIM_CHECK\",\"LIM_RUNTIME\"],",
            "\"diagnostic_hints\":\"advisory_not_automatic_edits\",",
            "\"commands\":{commands},",
            "\"semantic_index\":{semantic},",
            "\"sandbox\":{sandbox},",
            "\"sealed_source\":{sealed},",
            "\"types\":[\"int\",\"float\",\"string\",\"bool\",\"nil\",\"list\",\"map\",\"fn\"],",
            "\"keywords\":[\"let\",\"fn\",\"return\",\"if\",\"else\",\"while\",\"for\",\"in\",\"true\",\"false\",\"nil\",\"and\",\"or\",\"not\",\"break\",\"continue\"],",
            "\"operators\":[\"+\",\"-\",\"*\",\"/\",\"%\",\"==\",\"!=\",\"<\",\"<=\",\">\",\">=\",\"and\",\"or\",\"not\",\"..\",\"=\"],",
            "\"semantics\":{{\"falsy\":[\"nil\",\"false\"],\"short_circuit_returns_operand\":true,\"integers\":\"signed_64_checked\",\"collections\":\"shared_reference\",\"map_keys\":\"strings_sorted_iteration\",\"strings\":\"unicode_scalar_indexing\",\"closures\":\"capture_lexical_scope\"}},",
            "\"filesystem_builtins\":[],\"network_builtins\":[],\"stdout_builtin\":\"print\",",
            "\"builtins\":{builtins}}}"
        ),
        version=SCHEMA_VERSION,
        language_version=quote(env!("CARGO_PKG_VERSION")),
        steps=limits.max_steps,depth=limits.max_depth,source_limit=crate::lexer::MAX_SOURCE_BYTES,
        builtins=builtin_catalog_json(),
        commands=command_catalog_json(),
        semantic=r#"{"scope_model":"lexical_candidates","symbol_ids":"document_local","static_type_inference":false,"cross_file_index":false,"automatic_edits":false}"#,
        sandbox=r#"{"profiles":["unrestricted","restricted","pure"],"builtin_allowlist":true,"aggregate_output_bytes":true,"cooperative_timeout":true,"cooperative_cancellation_api":true,"interrupts_blocking_host_io":false,"os_isolation":false,"restricted_defaults":{"max_output_bytes":1048576,"timeout_ms":5000}}"#,
        sealed=r#"{"optional":true,"provider":"external_age","provider_availability":"requires_host_tool","ciphertext_extension":".lim.age","recipient_type":"native_X25519","ai_only_access":false,"plaintext_temporary_files":false,"plaintext_in_memory":true}"#,
    )
}

fn command_catalog_json() -> String {
    let commands = [
        ("run", "execute with explicit engine and host limits"),
        (
            "check",
            "report all static name and control-flow diagnostics",
        ),
        ("index", "build document-local lexical semantic index"),
        (
            "symbols",
            "list declarations, signatures and conservative captures",
        ),
        (
            "definition",
            "resolve declaration candidates at a source position",
        ),
        (
            "references",
            "find lexical reference candidates for a binding",
        ),
        (
            "complete",
            "complete visible bindings by prefix and position",
        ),
        (
            "capabilities",
            "discover versioned language and host interfaces",
        ),
        ("seal", "encrypt source for a native age recipient"),
        (
            "unseal",
            "authenticate and decrypt with a protected native age identity",
        ),
    ];
    let mut out = String::from("[");
    for (i, (name, purpose)) in commands.into_iter().enumerate() {
        if i != 0 {
            out.push(',');
        }
        write!(
            out,
            "{{\"name\":{},\"purpose\":{}}}",
            quote(name),
            quote(purpose)
        )
        .unwrap();
    }
    out.push(']');
    out
}
