//! Multi-file programs under tests/fixtures/ and examples/modules/.

use std::process::Command;

fn run(dir: &str, cmd: &str) -> (i32, String, String) {
    let root = env!("CARGO_MANIFEST_DIR");
    let o = Command::new(env!("CARGO_BIN_EXE_limits"))
        .arg(cmd)
        .arg(format!("{}/{}/main.lim", root, dir))
        .args(if cmd == "run" {
            &["--leak-check"][..]
        } else {
            &[]
        })
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
        "check error at 3:7: function 'hidden' in module 'lib' is private\n  \
         hint: add 'pub' to fn hidden in lib.lim\n\
         check error at 4:9: struct 'Secret' in module 'lib' is private\n  \
         hint: add 'pub' to struct Secret in lib.lim\n\
         check error at 5:7: module 'lib' has no function 'nope'\n\
         check error at 5:19: unknown module 'other' (missing 'import other'?)",
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
        "parse error in bad.lim at 2:9: expected identifier, found '='",
    );
    fails(
        "modcheck",
        "check error in m.lim at 2:18: value of 'y' must be str, got int",
    );
}

#[test]
fn runtime_error_in_module_names_the_file() {
    let (code, out, err) = run("tests/fixtures/modpanic", "run");
    assert_eq!((code, out.as_str()), (1, ""));
    assert_eq!(
        err,
        "runtime error in m/u.lim at 2:7: index 5 out of bounds for list of length 1"
    );
}

#[test]
fn inline_tests_in_modules_without_main() {
    let (code, out, err) = run("tests/fixtures/inline", "test");
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(out, "ok root\nok m.sq\n2 passed, 0 failed\n");
}

#[test]
fn module_capability_must_be_granted_by_root() {
    fails(
        "caps",
        "check error in b.lim at 1:6: capability 'env' is not declared by the root file (import chain: main.lim -> a.lim -> b.lim)\n  hint: add 'env' to the 'uses' line of the root file",
    );
}

#[test]
fn module_cannot_have_top_level_statements() {
    let dir = std::env::temp_dir().join(format!("limits-modtop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("main.lim"), "import m\nm.f()\n").unwrap();
    std::fs::write(dir.join("m.lim"), "pub fn f(): pass\nprint(1)\n").unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_limits"))
        .arg("check")
        .arg(dir.join("main.lim"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&o.stderr).trim_end(),
        "check error in m.lim at 2:1: a module cannot have top-level statements\n  \
         hint: move them into a function of this module"
    );
}

#[test]
fn enums_across_modules() {
    let (code, out, err) = run("tests/fixtures/enums", "run");
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        out,
        "[geo.tok.Num(1), geo.tok.Op(\"+\"), geo.tok.Num(22), geo.tok.End] 23 geo.tok.Num(1) geo.tok.Op(\"-\")\n"
    );
    fails(
        "enumpriv",
        "check error at 3:9: enum 'Hidden' in module 'tok' is private\n  \
         hint: add 'pub' to enum Hidden in tok.lim\n\
         check error at 4:8: enum 'Hidden' in module 'tok' is private\n  \
         hint: add 'pub' to enum Hidden in tok.lim\n\
         check error at 4:21: enum 'Hidden' in module 'tok' is private\n  \
         hint: add 'pub' to enum Hidden in tok.lim",
    );
}

/// Every .lim file in the repository (except the deliberately broken one and
/// the v0.1 `.dyn.lim` benchmark sources) and every code block of LLM.md
/// match grammar.ebnf. Needs python3; skipped without it.
#[test]
fn grammar_ebnf_accepts_all_sources_and_llm_examples() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 not found: skipped");
        return;
    }
    let dir = std::env::temp_dir().join(format!("limits-gram-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut files: Vec<String> = Vec::new();
    let llm = std::fs::read_to_string("LLM.md").unwrap();
    for (i, block) in llm.split("```\n").skip(1).step_by(2).enumerate() {
        let p = dir.join(format!("llm{}.lim", i));
        std::fs::write(&p, block).unwrap();
        files.push(p.to_string_lossy().into_owned());
    }
    assert!(files.len() >= 8, "LLM.md examples: {}", files.len());
    let mut todo = vec![
        std::path::PathBuf::from("examples"),
        "tests".into(),
        "bench".into(),
    ];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            let s = p.to_string_lossy().into_owned();
            if p.is_dir() {
                todo.push(p);
            } else if s.ends_with(".lim") && !s.ends_with(".dyn.lim") && !s.contains("modparse") {
                files.push(s);
            }
        }
    }
    let o = Command::new("python3")
        .args(["tools/grammar_check.py", "--grammar", "grammar.ebnf"])
        .args(&files)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stdout));
    // The checker does reject what the compiler rejects.
    let o = Command::new("python3")
        .args(["tools/grammar_check.py", "--grammar", "grammar.ebnf"])
        .arg("tests/fixtures/modparse/bad.lim")
        .output()
        .unwrap();
    assert!(!o.status.success());
}
