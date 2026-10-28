use limits::error::LimError;
use limits::interp::Limits;
use limits::{lexer, parser, resolve, run_with_engine, Engine};
use std::path::Path;

fn execute(src: &str, engine: Engine) -> (Vec<u8>, Result<(), LimError>) {
    let mut stdout = Vec::new();
    let result = run_with_engine(
        src,
        &mut stdout,
        Limits {
            max_steps: 10_000,
            max_depth: 32,
        },
        engine,
    );
    (stdout, result)
}

#[test]
fn agent_workflows_have_identical_verified_results() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/agents");
    let expected: &[(&str, &str)] = &[
        ("task_router.lim", "search 7\n"),
        ("dependency_plan.lim", "[\"research\", \"draft\", \"review\", \"publish\"]\n"),
        ("retry_policy.lim", "{\"attempts\": 3, \"backoff_ticks\": 3, \"done\": true}\n"),
        ("record_transform.lim", "{\"by_status\": {\"done\": 2, \"queued\": 1}, \"cost_units\": 48, \"ids\": [\"a\", \"b\", \"c\"]}\n"),
        ("closure_budget.lim", "0 2\n"),
        ("bounded_search.lim", "[\"start\", \"issues\", \"done\"]\n"),
    ];
    let mut actual_files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "lim"))
        .collect();
    actual_files.sort();
    let mut expected_files: Vec<_> = expected.iter().map(|(name, _)| dir.join(name)).collect();
    expected_files.sort();
    assert_eq!(
        actual_files, expected_files,
        "every workflow needs an expected result"
    );
    for (name, stdout) in expected {
        let source = std::fs::read_to_string(dir.join(name)).unwrap();
        let program = parser::parse(lexer::lex(&source).unwrap()).unwrap();
        assert_eq!(resolve::check(&program), Vec::new(), "{name}");
        let tree = execute(&source, Engine::Tree);
        let vm = execute(&source, Engine::Vm);
        assert_eq!(tree, vm, "{name}");
        assert_eq!(tree.0, stdout.as_bytes(), "{name}");
        tree.1.unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}
