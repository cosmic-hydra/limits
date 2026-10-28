//! Value semantics, reference counting and memory: every program in the
//! tree frees all its memory (`--leak-check`), early exits release what they
//! hold, long chains free without recursion, `--max-mem` counts live bytes,
//! and `inout` / `&` are checked.

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

/// Run `limits <cmd> t.lim [flags] --leak-check`: (stdout, stderr, exit code).
fn limits(cmd: &str, src: &str, flags: &[&str]) -> (String, String, i32) {
    let dir = std::env::temp_dir().join(format!(
        "limits-mem-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("t.lim"), src).unwrap();
    let mut c = Command::new(env!("CARGO_BIN_EXE_limits"));
    c.arg(cmd).arg(dir.join("t.lim")).args(flags);
    if cmd != "check" {
        c.arg("--leak-check");
    }
    let o = c.output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}

/// Runs checked and unchecked, leak-checked, and expects exactly `want`.
fn ok(src: &str, want: &str) {
    for flags in [&[][..], &["--unchecked"][..]] {
        let (out, err, code) = limits("run", src, flags);
        assert_eq!(err, "", "stderr ({:?})", flags);
        assert_eq!(code, 0, "{:?}", flags);
        assert_eq!(out, want, "{:?}", flags);
    }
}

fn check_fails(src: &str, want: &str) {
    let (_, err, code) = limits("check", src, &[]);
    assert_eq!(code, 1, "stderr: {}", err);
    assert_eq!(err.trim_end(), want);
}

/// Every example, benchmark and agent task (and the modules fixture
/// programs that run) ends with zero live heap blocks.
#[test]
fn every_program_in_the_tree_frees_all_memory() {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut files = Vec::new();
    for dir in [
        "examples",
        "bench",
        "bench/agent_tasks",
        "bench/agent_tasks/before",
    ] {
        for e in std::fs::read_dir(format!("{}/{}", root, dir)).unwrap() {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            if name.ends_with(".lim") && !name.ends_with(".dyn.lim") {
                files.push(p);
            }
        }
    }
    files.push(format!("{}/examples/modules/main.lim", root).into());
    for f in ["diamond", "collide", "enums", "inline"] {
        files.push(format!("{}/tests/fixtures/{}/main.lim", root, f).into());
    }
    files.sort();
    let files: Vec<(std::path::PathBuf, &str)> = files
        .into_iter()
        .map(|f| {
            // The inline fixture has only tests: run them (each leak-checked).
            let cmd = if f.to_string_lossy().contains("fixtures/inline") {
                "test"
            } else {
                "run"
            };
            (f, cmd)
        })
        .collect();
    assert!(files.len() > 30, "found only {} programs", files.len());
    // Run them in parallel: several are benchmarks taking a second or so.
    let failures: Vec<String> = std::thread::scope(|s| {
        let kids: Vec<_> = files
            .chunks(files.len().div_ceil(4))
            .map(|chunk| {
                s.spawn(move || {
                    let mut bad = Vec::new();
                    for (f, cmd) in chunk {
                        let o = Command::new(env!("CARGO_BIN_EXE_limits"))
                            .arg(cmd)
                            .arg(f)
                            .args(["--leak-check", "--max-steps", "100000000000"])
                            .output()
                            .unwrap();
                        let err = String::from_utf8_lossy(&o.stderr);
                        if !o.status.success() || (!err.is_empty() && *cmd == "run") {
                            bad.push(format!("{}: {}", f.display(), err));
                        }
                    }
                    bad
                })
            })
            .collect();
        kids.into_iter().flat_map(|k| k.join().unwrap()).collect()
    });
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn leak_check_flag_is_compiled_in() {
    let (out, _, code) = limits("build", "print(1)", &["--emit-c"]);
    assert_eq!(code, 0);
    assert!(out.contains("leak check: %lld heap blocks"));
    let (out, _, _) = {
        let o = Command::new(env!("CARGO_BIN_EXE_limits"))
            .args(["build", "/dev/null", "--emit-c"])
            .output()
            .unwrap();
        (String::from_utf8_lossy(&o.stdout).into_owned(), (), ())
    };
    assert!(!out.contains("leak check:"));
}

#[test]
fn early_exits_release_what_they_hold() {
    let src = r#"
fn first_long(xs: [str]) -> str:
    for x in xs:
        let y = x + "!"
        if len(y) > 3:
            return y
    "none"
fn parse_all(xs: [str]) -> Option[[int]]:
    let tag = "parsing " + str(len(xs))
    Some([parse_int(x)? * 2 for x in xs if x != tag])
fn total(r: Result[[str], str]) -> Result[int, str]:
    let xs = r?
    Ok(len(xs))
fn find_word(words: [str], w: str) -> int:
    var i = 0
    while i < len(words):
        let cur = [words[i], w]
        match cur[0] == cur[1]:
            true: return i
            false: pass
        i += 1
    -1
var kept: [[str]] = []
for i in 0..5:
    let row = [str(i), "x"]
    if i == 1:
        continue
    if i == 3:
        break
    push(kept, row)
let v = match parse_int("12"):
    Some(n):
        let s = str(n) + "?"
        s
    None: "no"
print(first_long(["a", "bb", "cccc", "dd"]), parse_all(["1", "2"]), parse_all(["1", "x", "3"]))
print(total(Ok(["a"])), total(Err("bad")), find_word(["p", "q", "r"], "q"), kept, v)
"#;
    ok(
        src,
        "cccc! Some([2, 4]) None\nOk(1) Err(\"bad\") 1 [[\"0\", \"x\"], [\"2\", \"x\"]] 12?\n",
    );
}

#[test]
fn assignment_copies_and_writes_copy_on_write() {
    let src = r#"
struct Team:
    name: str
    members: [str]
var a = Team { name: "a", members: ["x"] }
var b = a
push(b.members, "y")
b.name += "2"
var teams = [a, b]
let snapshot = teams
teams[0].members[0] = "z"
for t in teams:
    push(teams, t)
var s = "ab"
let t = s
s += "c"
var xs = [1, 2, 3]
let ys = xs
xs[0] = 9
pop(xs)
print(a, b)
print(len(teams), teams[0].members, snapshot[0].members, s, t, xs, ys)
"#;
    ok(
        src,
        "Team {name: \"a\", members: [\"x\"]} Team {name: \"a2\", members: [\"x\", \"y\"]}\n4 [\"z\"] [\"x\"] abc ab [9, 2] [1, 2, 3]\n",
    );
}

#[test]
fn inout_parameters() {
    let src = r#"
struct Acc:
    n: int
    log: [str]
fn add(a: inout Acc, k: int):
    a.n += k
    push(a.log, "+" + str(k))
fn reset(xs: inout [int]):
    xs = []
fn twice(a: inout Acc):
    add(&a, 1)
    add(&a, 2)
fn swap(a: inout int, b: inout int):
    let t = a
    a = b
    b = t
var acc = Acc { n: 0, log: [] }
twice(&acc)
var accs = [acc, acc]
add(&accs[1], 10)
var xs = [1, 2]
let keep = xs
reset(&xs)
var p = 1
var q = 2
swap(&p, &q)
fn grow(xs: inout [int], v: [int]):
    for x in v:
        push(xs, x)
var g = [1]
grow(&g, g)
print(acc, accs[1].n, accs[0].n, xs, keep, p, q, g)
"#;
    ok(
        src,
        "Acc {n: 3, log: [\"+1\", \"+2\"]} 13 3 [] [1, 2] 2 1 [1, 1]\n",
    );
}

#[test]
fn inout_and_mutation_errors_have_hints() {
    check_fails(
        "fn f(xs: inout [int]): push(xs, 1)\nfn g(xs: [int]): pass\nvar a = [1]\nf(a)\ng(&a)\nlet b = [2]\nf(&b)\nfn h(x: inout int, y: inout int): pass\nvar n = 1\nh(&n, &n)\nf(&[1])\n",
        "check error at 4:3: argument 1 of 'f' is inout (parameter 'xs'), so it must be passed as '&a'\n  \
         hint: write '&a' to let 'f' change it\n\
         check error at 5:3: '&' passes a variable to an inout parameter, but parameter 'xs' of 'g' is not inout\n  \
         hint: remove the '&' (arguments are passed read-only), or declare the parameter 'xs: inout [int]'\n\
         check error at 7:4: cannot change 'b': it is declared with 'let'\n  \
         hint: declare it with 'var b = ...' to allow changes\n\
         check error at 10:7: 'n' is passed to two inout parameters in one call\n  \
         hint: pass it once and change the parts in separate calls\n\
         check error at 11:4: can only change a variable, or a field or element of one\n  \
         hint: store the value in a variable first: var xs = ...",
    );
    check_fails(
        "struct P:\n    x: int\nfn f(p: P):\n    p.x = 1\nfor q in [P { x: 1 }]:\n    q.x = 2\nprint(&1)\n",
        "check error at 4:6: cannot change parameter 'p': parameters are read-only\n  \
         hint: to change the caller's value declare 'p: inout P' and call with '&p'; to change a local copy start the body with 'var p = p'\n\
         check error at 6:6: cannot change loop variable 'q': it is a copy of the element\n  \
         hint: change the element itself instead, e.g. 'xs[i] = ...' in 'for i in 0..len(xs)'\n\
         check error at 7:7: '&' can only pass a variable to an 'inout' parameter of a fn\n  \
         hint: remove the '&'",
    );
    let (_, err, code) = limits("check", "let x = &y\n", &[]);
    assert_eq!(code, 1);
    assert!(
        err.contains("'&x' is only allowed as a call argument"),
        "{}",
        err
    );
    let (_, err, _) = limits(
        "check",
        "fn f(xs: inout [int]): pass\nf([1])\n",
        &["--json"],
    );
    assert!(err.contains("\"code\":\"E0225\""), "{}", err);
    let (_, err, _) = limits("check", "let xs = [1]\npush(xs, 2)\n", &["--json"]);
    assert!(err.contains("\"code\":\"E0222\""), "{}", err);
}

#[test]
fn long_chains_free_without_recursion() {
    // Dropping a 1e6-long linked list or a deep tree must not use the C
    // stack per element (it would overflow it).
    let src = r#"
enum Chain:
    Link(v: int, next: Chain)
    End
struct Tree:
    v: int
    kids: [Tree]
var c = End
for i in 0..1000000:
    c = Link(i, c)
var t = Tree { v: 0, kids: [] }
for i in 0..300000:
    t = Tree { v: i, kids: [t] }
let n = match c:
    Link(v, _): v
    End: -1
c = End
print(n, t.v)
"#;
    ok(src, "999999 299999\n");
}

#[test]
fn max_mem_counts_live_bytes() {
    // 2e6 strings are built and freed again: live memory stays tiny.
    let src = "var n = 0\nfor i in 0..2000000:\n    let s = \"item\" + str(i)\n    n += len(s)\nvar xs = [0]\nfor i in 0..100000:\n    xs = [i, i + 1]\nprint(n, xs)\n";
    let (out, err, code) = limits("run", src, &["--max-mem", "1"]);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("20888890 [99999, 100000]\n", "", 0)
    );
    // ... but keeping them all does not fit.
    let keep =
        "var xs: [str] = []\nfor i in 0..2000000: push(xs, \"item\" + str(i))\nprint(len(xs))\n";
    let (_, err, code) = limits("run", keep, &["--max-mem", "8"]);
    assert_eq!(code, 1);
    assert!(err.contains("memory limit exceeded"), "{}", err);
}

#[test]
fn string_appends_are_linear() {
    // 2e5 appends of a growing string: quadratic copying would take
    // minutes and exceed the step budget for byte copies.
    let src = "var s = \"\"\nfor i in 0..200000:\n    s = s + str(i % 10) + \",\"\nvar t = \"\"\nfor i in 0..200000: t += \"ab\"\nprint(len(s), len(t))\n";
    let (out, err, code) = limits("run", src, &["--max-steps", "5000000"]);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        ("400000 400000\n", "", 0)
    );
}

#[test]
fn reading_a_variable_while_a_call_changes_it() {
    // An argument that reads `xs` stays valid while another argument (or
    // the call itself) changes `xs` through `&`.
    let src = r#"
fn clear(xs: inout [str]) -> int:
    let n = len(xs)
    xs = []
    n
fn both(a: [str], n: int) -> str: join(a, ",") + "/" + str(n)
var xs = ["a", "b"]
print(both(xs, clear(&xs)), xs)
var ys = ["p", "q"]
print(ys[0] + str(clear(&ys)), ys)
fn tail(xs: inout [str], head: str) -> str:
    push(xs, head)
    xs[0]
var zs = ["m"]
print(tail(&zs, zs[0]), zs)
"#;
    ok(src, "a,b/2 []\np2 []\nm [\"m\", \"m\"]\n");
}

#[test]
fn write_caching_and_borrowed_lets_keep_value_semantics() {
    // Repeated writes through one path check uniqueness once; anything that
    // could share the value in between (a heap `let`, a changed index, a
    // call with `&`) must check again.
    let src = r#"
struct P:
    x: int
    tags: [int]
fn keep(xs: inout [[int]], v: [int]):
    push(xs, v)
var xs = [[1, 2], [3, 4]]
xs[0][0] = 10
let snap = xs
xs[0][1] = 20
xs[1][0] = 30
var i = 0
xs[i][1] = 7
i = 1
xs[i][1] = 8
var kept: [[int]] = []
keep(&kept, xs[0])
xs[0][0] = 99
var ps = [P { x: 1, tags: [1] }]
let first = ps[0]
ps[0].x = 2
ps[0].tags[0] = 5
fn sum_tags(p: P) -> int: sum(p.tags)
var total = 0
for k in 0..3:
    let p = ps[0]
    total += p.x + sum_tags(p)
    ps[0].x += 1
print(xs, snap, kept, first, ps, total)
"#;
    ok(
        src,
        "[[99, 7], [30, 8]] [[10, 2], [3, 4]] [[10, 7]] P {x: 1, tags: [1]} [P {x: 5, tags: [5]}] 24\n",
    );
}

#[test]
fn early_return_inside_constructor_operands_frees_them() {
    let src = r#"
struct Pair:
    a: [int]
    b: [int]
fn pair(x: str, y: str) -> Option[Pair]:
    Some(Pair { a: [1, 2, 3], b: [parse_int(x)?, parse_int(y)?] })
fn lst(x: str) -> Option[[[int]]]:
    Some([[1, 2], [parse_int(x)?], [3]])
print(pair("1", "2"), pair("1", "z"), lst("4"), lst("q"))
"#;
    ok(
        src,
        "Some(Pair {a: [1, 2, 3], b: [1, 2]}) None Some([[1, 2], [4], [3]]) None\n",
    );
}

#[test]
fn loop_writes_stay_copy_on_write() {
    // Loops that write through a variable check it is unshared once, before
    // the loop; values shared before or inside the loop stay unchanged.
    let src = r#"
struct P:
    x: int
    tags: [int]
var a = [1, 2]
var snaps: [[int]] = []
for i in 0..3:
    push(snaps, a)
    a[0] += 1
var b = [0, 0]
let b0 = b
for i in 0..2:
    b[i] = 5
var p = P { x: 0, tags: [] }
let q = p
var i = 0
while i < 3:
    p.x += 1
    push(p.tags, i)
    i += 1
var grid = [[0, 0], [1, 1]]
var rows: [[int]] = []
for r in 0..2:
    push(rows, grid[r])
    grid[r][0] = 9
fn fill(xs: inout [int], n: int):
    for k in 0..n:
        push(xs, len(xs))
var c = [7]
let c0 = c
fill(&c, 3)
print(snaps, a, b, b0, p, q)
print(grid, rows, c, c0)
"#;
    ok(
        src,
        "[[1, 2], [2, 2], [3, 2]] [4, 2] [5, 5] [0, 0] P {x: 3, tags: [0, 1, 2]} P {x: 0, tags: []}\n[[9, 0], [9, 1]] [[0, 0], [1, 1]] [7, 1, 2, 3] [7]\n",
    );
}
