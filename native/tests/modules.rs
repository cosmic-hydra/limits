//! Multi-file programs under tests/fixtures/ and examples/modules/.

use std::process::Command;

fn run(dir: &str, cmd: &str) -> (i32, String, String) {
    let root = env!("CARGO_MANIFEST_DIR");
    let o = Command::new(env!("CARGO_BIN_EXE_limits"))
        .arg(cmd)
        .arg(format!("{}/{}/main.lim", root, dir))
        .output()
        .unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).trim_end().to_string(),
    )
}

fn fails(dir: &str, want: &str) {
    let (code, out, err) = run(&format!("tests/fixtures/{}", dir), "check");
    assert_eq!(code, 1, "stdout: {}", out);
    assert_eq!(err, want);
}

#[test]
fn same_names_in_different_modules_do_not_collide() {
    let (code, out, err) = run("tests/fixtures/collide", "run");
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(out, "root a b tricky\n1 two 30 1!\na.P {v: \"two\"}\n");
}

#[test]
fn diamond_imports_and_nested_paths() {
    let (code, out, err) = run("tests/fixtures/diamond", "run");
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(out, "11 shared.Counter {n: 5}\n");
}

#[test]
fn modules_example() {
    let (code, out, err) = run("examples/modules", "run");
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(out, "len 5.0\narea 25 1000\nrect(5x5)\n");
}

#[test]
fn import_cycle_prints_path() {
    fails(
        "cycle",
        "check error in b.lim at 1:1: import cycle: a.lim -> b.lim -> a.lim",
    );
}

#[test]
fn private_and_unknown_items() {
    fails(
        "private",
        "check error at 4:9: function 'hidden' in module 'lib' is private (mark it 'pub')\n\
         check error at 5:11: struct 'Secret' in module 'lib' is private (mark it 'pub')\n\
         check error at 6:9: module 'lib' has no function 'nope'\n\
         check error at 6:21: unknown module 'other' (missing 'import other;'?)",
    );
}

#[test]
fn missing_module_file() {
    fails(
        "missing",
        "check error at 1:1: cannot find module 'util.nothere' (no file util/nothere.lim)",
    );
}

#[test]
fn errors_inside_modules_name_the_file() {
    fails(
        "modparse",
        "parse error in bad.lim at 2:7: expected identifier, found '='",
    );
    fails(
        "modcheck",
        "check error in m.lim at 2:16: value of 'y' must be str, got int",
    );
}

#[test]
fn runtime_error_in_module_names_the_file() {
    let (code, out, err) = run("tests/fixtures/modpanic", "run");
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(
        err,
        "runtime error in m/u.lim at 2:12: index 5 out of bounds for list of length 1"
    );
}
