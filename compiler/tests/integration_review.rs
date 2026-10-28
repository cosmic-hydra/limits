//! Independent review regressions for agent-facing error-envelope consistency.
use std::process::Command;

fn host_or_usage_error(args: &[&str]) -> String {
    let result = Command::new(env!("CARGO_BIN_EXE_lim"))
        .args(args)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stderr.is_empty());
    let json = String::from_utf8(result.stdout).unwrap();
    assert!(json.contains("\"schema_version\":1"), "{json}");
    assert!(json.contains("\"command\":\"run\""), "{json}");
    assert!(json.contains("\"ok\":false"), "{json}");
    assert!(json.contains("\"diagnostic_count\":1"), "{json}");
    assert!(json.contains("\"diagnostics_truncated\":false"), "{json}");
    assert!(json.contains("\"stdout\":\"\""), "{json}");
    // Null means no execution started and no engine provenance is claimed.
    assert!(json.contains("\"engine\":null"), "{json}");
    json
}

#[test]
fn run_host_errors_keep_common_json_metadata() {
    let missing =
        std::env::temp_dir().join(format!("limits-review-missing-{}.lim", std::process::id()));
    assert!(!missing.exists());
    let json = host_or_usage_error(&["run", missing.to_str().unwrap(), "--json"]);
    assert!(json.contains("\"code\":\"LIM_HOST\""));
    assert!(json.contains("\"kind\":\"host\""));
}

#[test]
fn run_usage_errors_keep_common_json_metadata() {
    let json = host_or_usage_error(&["run", "-", "--json", "--engine", "invalid"]);
    assert!(json.contains("\"code\":\"LIM_USAGE\""));
    assert!(json.contains("\"kind\":\"usage\""));
}
