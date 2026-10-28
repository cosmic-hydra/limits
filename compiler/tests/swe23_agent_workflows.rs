//! Agent discovery and a real report-helper repair cycle. JSON encoding itself
//! is independently decoded in agent_cli.rs; these checks exercise its consumer
//! contract without another JSON implementation or external runtime dependency.
use std::io::Write;
use std::process::{Command, Stdio};

fn invoke(args: &[&str], source: &[u8]) -> (i32, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lim"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(source).unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.stderr.is_empty(), "{result:?}");
    let response = String::from_utf8(result.stdout).unwrap();
    assert_eq!(response.lines().count(), 1, "{response}");
    assert!(response.starts_with("{\"schema_version\":1,"), "{response}");
    (result.status.code().unwrap(), response)
}

#[test]
fn discovery_advertises_every_cli_diagnostic_category() {
    let (status, catalog) = invoke(&["capabilities", "--json"], b"");
    assert_eq!(status, 0);
    let codes = catalog
        .split_once("\"diagnostic_codes\":[")
        .unwrap()
        .1
        .split_once(']')
        .unwrap()
        .0;
    for (args, source, status, code) in [
        (vec!["check", "-", "--json"], "@", 1, "LIM_LEX"),
        (vec!["check", "-", "--json"], "let x=;", 1, "LIM_PARSE"),
        (vec!["check", "-", "--json"], "missing;", 1, "LIM_CHECK"),
        (
            vec!["run", "-", "--engine", "vm", "--json"],
            "sum([false]);",
            1,
            "LIM_RUNTIME",
        ),
        (
            vec!["run", "-", "--engine", "invalid", "--json"],
            "",
            2,
            "LIM_USAGE",
        ),
        (
            vec!["run", "-", "--json"],
            // The bytes passed below are invalid UTF-8, so no source is run.
            "invalid UTF-8",
            2,
            "LIM_HOST",
        ),
    ] {
        let input = if code == "LIM_HOST" {
            &[0xff][..]
        } else {
            source.as_bytes()
        };
        let (actual_status, response) = invoke(&args, input);
        assert_eq!(actual_status, status, "{response}");
        assert!(
            response.contains(&format!("\"code\":\"{code}\"")),
            "{response}"
        );
        assert!(
            codes.split(',').any(|item| item == format!("\"{code}\"")),
            "wire diagnostic {code} is absent from discovery: {codes}"
        );
    }
}

#[test]
fn report_repair_preserves_present_nil_and_the_original_execution_limits() {
    let fixture = include_str!("../../bench/agent_compact/report_helpers.lim");
    let original = "let label = get(job, \"title\", \"untitled\");";
    assert!(fixture.contains(original), "report fixture changed");
    let eager = "let label = get(job, \"title\", fallback());";
    let candidate = format!(
        "let fallback_calls = [];\nfn fallback() {{ push(fallback_calls, true); return \"untitled\"; }}\n{}",
        fixture
            .replace("let job = {\"attempt\": nil};", "let job = {\"attempt\": nil, \"title\": nil};")
            .replace(original, eager)
            .replace("print(label,", "assert(len(fallback_calls) == 0, \"fallback ran for present key\");\nprint(label,")
    );
    // This minimal correction tests membership rather than truthiness or nil.
    // In particular, the present title must remain nil instead of becoming the
    // fallback text, and the fallback's side effect must not run.
    let repaired = candidate.replace(
        eager,
        "let label = job[\"title\"];\nif not has(job, \"title\") { label = fallback(); }",
    );
    for source in [&candidate, &repaired] {
        let (status, check) = invoke(&["check", "-", "--json"], source.as_bytes());
        assert_eq!(status, 0, "{check}");
        assert!(check.contains("\"diagnostic_count\":0"), "{check}");
    }
    for engine in ["tree", "vm"] {
        let args = [
            "run",
            "-",
            "--engine",
            engine,
            "--json",
            "--sandbox",
            "restricted",
            "--max-steps",
            "10000",
            "--max-depth",
            "32",
            "--max-output-bytes",
            "128",
            "--timeout-ms",
            "1000",
        ];
        let (status, failed) = invoke(&args, candidate.as_bytes());
        assert_eq!(status, 1, "{failed}");
        assert!(failed.contains("\"code\":\"LIM_RUNTIME\""), "{failed}");
        assert!(failed.contains("fallback ran for present key"), "{failed}");
        assert!(failed.contains("\"stdout\":\"\""), "{failed}");
        let (status, success) = invoke(&args, repaired.as_bytes());
        assert_eq!(status, 0, "{success}");
        assert!(success.contains("\"diagnostic_count\":0"), "{success}");
        assert!(
            success.contains("\"stdout\":\"nil true search -> read -> answer 9\\n\""),
            "{success}"
        );
    }
}
