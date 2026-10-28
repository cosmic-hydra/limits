use limits::error::{Kind, LimError, Span};
use limits::protocol::{self, SCHEMA_VERSION};
use limits::Engine;

#[test]
fn quotes_every_ascii_control_without_changing_unicode() {
    assert_eq!(
        protocol::quote("\"\\\n\r\t\u{8}\u{c}"),
        r#""\"\\\n\r\t\b\f""#
    );
    assert_eq!(protocol::quote("\0\u{1}\u{1f}"), r#""\u0000\u0001\u001f""#);
    assert_eq!(
        protocol::quote("héllo 🦀\u{2028}\u{2029}"),
        "\"héllo 🦀\\u2028\\u2029\""
    );
    let controls: String = (0..=31).map(char::from).collect();
    let quoted = protocol::quote(&controls);
    assert!(quoted.chars().all(|c| c >= ' '));
    assert!(quoted.starts_with('"') && quoted.ends_with('"'));
}

#[test]
fn diagnostic_preserves_message_and_source_coordinates() {
    let error = LimError::new(
        Kind::Runtime,
        "undefined variable 'x'",
        Span { line: 3, col: 9 },
    );
    let json = protocol::diagnostic_json(&error);
    assert!(json.contains(r#""code":"LIM_RUNTIME""#));
    assert!(json.contains(r#""severity":"error""#));
    assert!(json.contains(r#""line":3,"col":9"#));
    assert!(json.contains(r#""message":"undefined variable 'x'""#));
    assert!(json.contains("ensure a visible declaration executes"));
}

#[test]
fn malicious_error_text_cannot_change_code_or_inject_fields() {
    let error = LimError::new(
        Kind::Runtime,
        "execution cancelled\",\"ok\":true,\"message\":\"",
        Span { line: 1, col: 1 },
    );
    let json = protocol::diagnostic_json(&error);
    assert_eq!(protocol::diagnostic_code(&error), "LIM_RUNTIME");
    assert!(json.contains(r#"cancelled\",\"ok\":true,\"message\":\""#));
    assert!(!json.contains(r#"cancelled","ok":true"#));
    for message in [
        "execution cancelled",
        "execution deadline exceeded",
        "sandbox denied builtin 'print'",
    ] {
        assert_eq!(
            protocol::diagnostic_code(&LimError::new(
                Kind::Runtime,
                message,
                Span { line: 1, col: 1 }
            )),
            "LIM_RUNTIME"
        );
    }
}

#[test]
fn check_can_return_multiple_errors_and_no_source() {
    let errors = [
        LimError::new(
            Kind::Check,
            "undefined variable 'x'",
            Span { line: 1, col: 7 },
        ),
        LimError::new(
            Kind::Check,
            "'break' outside of a loop",
            Span { line: 2, col: 1 },
        ),
    ];
    let response = protocol::check_response(None, &errors);
    assert!(response.starts_with(&format!("{{\"schema_version\":{SCHEMA_VERSION},")));
    assert!(response.contains(r#""command":"check","ok":false,"source":null"#));
    assert_eq!(response.matches(r#""code":"LIM_CHECK""#).count(), 2);
    assert!(response.ends_with(r#"],"data":null}"#));
    assert!(protocol::check_response(Some("a.lim"), &[]).contains(r#""ok":true,"source":"a.lim""#));
}

#[test]
fn run_preserves_partial_stdout_on_error_and_engine() {
    let error = LimError::new(Kind::Runtime, "division by zero", Span { line: 2, col: 8 });
    let response = protocol::run_response(Some("a\"b.lim"), Engine::Vm, "first\n", &Err(error));
    assert!(response.contains(r#""ok":false"#));
    assert!(response.contains(r#""source":"a\"b.lim""#));
    assert!(response.ends_with(r#""engine":"vm","stdout":"first\n"}"#));
    let response = protocol::run_response(None, Engine::Tree, "", &Ok(()));
    assert!(response.contains(r#""ok":true"#));
    assert!(response.contains(r#""diagnostics":[],"engine":"tree","stdout":"""#));
}

#[test]
fn command_wrapper_escapes_metadata_and_preserves_serialized_data() {
    let response =
        protocol::command_response("symbols\"", Some("x\ny.lim"), r#"{"symbols":[]}"#, &[]);
    assert!(response.contains(r#""command":"symbols\"""#));
    assert!(response.contains(r#""source":"x\ny.lim""#));
    assert!(response.ends_with(r#""data":{"symbols":[]}}"#));
}

#[test]
fn capabilities_match_builtin_registry_and_limits() {
    let json = protocol::capabilities_json();
    for b in limits::interp::BUILTINS {
        assert!(
            json.contains(&format!("\"name\":{}", protocol::quote(b))),
            "{b}"
        );
    }
    assert_eq!(
        json.matches(r#""signature":""#).count(),
        limits::interp::BUILTINS.len()
    );
    assert!(json.contains(r#""max_steps":10000000,"max_depth":256"#));
    assert!(json.contains(r#""column_unit":"unicode_scalar""#));
    assert!(json.contains(r#""filesystem_builtins":[],"network_builtins":[]"#));
    for command in [
        "run",
        "check",
        "index",
        "symbols",
        "definition",
        "references",
        "complete",
        "capabilities",
        "seal",
        "unseal",
    ] {
        assert!(json.contains(&format!("\"name\":{}", protocol::quote(command))));
    }
    assert!(json.contains(r#""os_isolation":false"#));
    assert!(json.contains(r#""ai_only_access":false"#));
    assert!(json.contains(r#""provider_availability":"requires_host_tool""#));
    assert!(!json.contains("military"));
    assert!(!json.contains("AI-only"));
}

#[test]
fn oversized_messages_are_explicitly_truncated_at_utf8_boundary() {
    let message = format!(
        "{}🦀suffix",
        "x".repeat(protocol::MAX_DIAGNOSTIC_MESSAGE_BYTES - 1)
    );
    let error = LimError::new(Kind::Runtime, message.clone(), Span { line: 1, col: 2 });
    let encoded = protocol::diagnostic_json(&error);
    assert!(encoded.contains(r#""message_truncated":true"#));
    assert!(encoded.contains(&format!("\"original_message_bytes\":{}", message.len())));
    assert!(!encoded.contains("🦀"));
    assert!(!encoded.contains("suffix"));
    assert_eq!(error.msg, message);
    let ordinary = protocol::diagnostic_json(&LimError::new(
        Kind::Parse,
        "small",
        Span { line: 1, col: 1 },
    ));
    assert!(ordinary.contains(r#""message":"small""#));
    assert!(ordinary.contains(r#""message_truncated":false,"original_message_bytes":5"#));
}
#[test]
fn run_frontend_failures_keep_the_run_envelope_and_all_diagnostics() {
    let errors = [
        LimError::new(
            Kind::Check,
            "undefined variable 'a'",
            Span { line: 1, col: 7 },
        ),
        LimError::new(
            Kind::Check,
            "undefined variable 'b'",
            Span { line: 1, col: 9 },
        ),
    ];
    let response = protocol::run_errors_response(Some("a.lim"), Engine::Vm, "", &errors);
    assert!(response.contains(r#""command":"run","ok":false"#));
    assert_eq!(response.matches(r#""code":"LIM_CHECK""#).count(), 2);
    assert!(response.ends_with(r#""engine":"vm","stdout":""}"#));
}

#[test]
fn aggregate_diagnostics_are_capped_without_losing_total_count() {
    let errors: Vec<_> = (0..protocol::MAX_DIAGNOSTICS + 19)
        .map(|i| {
            LimError::new(
                Kind::Check,
                format!("undefined variable 'x{i}'"),
                Span {
                    line: i + 1,
                    col: 1,
                },
            )
        })
        .collect();
    for response in [
        protocol::check_response(None, &errors),
        protocol::command_response("index", None, "null", &errors),
        protocol::run_errors_response(None, Engine::Tree, "", &errors),
    ] {
        assert!(response.contains(&format!("\"diagnostic_count\":{}", errors.len())));
        assert!(response.contains(r#""diagnostics_truncated":true"#));
        assert_eq!(
            response.matches(r#""code":"LIM_CHECK""#).count(),
            protocol::MAX_DIAGNOSTICS
        );
        assert!(!response.contains("x1042"));
    }
}
