//! Exercise native filesystem arguments and byte-oriented source handling on
//! every CI host without requiring a shell or an external executable.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        for _ in 0..100 {
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("limits-swe16-{}-{id}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create test directory: {error}"),
            }
        }
        panic!("cannot allocate test directory");
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn cli(directory: &Path, command: &str, source: &Path, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lim"))
        .current_dir(directory)
        .arg(command)
        .arg(source)
        .args(flags)
        .output()
        .unwrap()
}

fn success(output: Output) -> String {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn unicode_and_space_paths_execute_identically_with_mixed_line_endings() {
    let temp = Temp::new();
    let filename = Path::new("source café 名 😀 with spaces.lim");
    let absolute = temp.0.join(filename);
    std::fs::write(
        &absolute,
        "let 名=\"😀 café\";\r\nlet map={\"z\":3,\"a\":1};\rprint(名,map);\n",
    )
    .unwrap();

    for engine in ["tree", "vm"] {
        for path in [filename, absolute.as_path()] {
            let output = success(cli(&temp.0, "run", path, &["--engine", engine]));
            assert_eq!(output, "😀 café {\"a\": 1, \"z\": 3}\n");
        }
    }
}

#[test]
fn file_and_stdin_indexing_have_identical_payloads_and_escaped_native_paths() {
    let temp = Temp::new();
    let absolute = temp.0.join("index café 名 😀 with spaces.lim");
    let source = "let 名=1;\r\nfn helper(参数){return 参数+名;}\rprint(helper(名));\n";
    std::fs::write(&absolute, source.as_bytes()).unwrap();
    let file = success(cli(&temp.0, "index", &absolute, &["--json"]));

    let mut child = Command::new(env!("CARGO_BIN_EXE_lim"))
        .args(["index", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    let stdin = success(child.wait_with_output().unwrap());
    let source_field = format!(
        "\"source\":{}",
        limits::protocol::quote(absolute.to_str().unwrap())
    );
    assert_eq!(file.matches(&source_field).count(), 1);
    assert_eq!(file.replacen(&source_field, "\"source\":\"-\"", 1), stdin);
    assert!(file.ends_with("}\n"));
    assert_eq!(file.lines().count(), 1);
}
