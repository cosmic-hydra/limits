#!/usr/bin/env python3
"""Re-run every Python->Limits pair and check the outputs agree.

Each pair lives in pairs/<name>/:
  prog.py, prog.lim          the two programs (plus any module files they import)
  tests/cases.json           [{"name", "stdin", "args": [...], "files": {path: text},
                              "env": {NAME: value}}]
  tests/<case>.out           expected stdout

For every case the Python program and the compiled Limits program are run in
a fresh temp directory holding `files`, with `stdin` and `args`; a case passes
only when both outputs equal <case>.out byte for byte. Pairs whose prog.lim has
inline `test` blocks must also pass `limits test`.

  python3 verify.py                  verify all pairs, rewrite index.jsonl
  python3 verify.py NAME...          verify only these pairs (index untouched)
  python3 verify.py --expect NAME... (re)write tests/*.out from the Python
                                     program (never used for bench_* pairs,
                                     whose .out files come from the task JSON)
Exit status 0 only when every selected pair passes.
"""
import json, os, re, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
PAIRS = os.path.join(HERE, "pairs")
LIMITS = os.environ.get("LIMITS", os.path.join(HERE, "..", "native", "target", "release", "limits"))
TIMEOUT = 60

FEATURES = [  # (name, regex over prog.lim with comments and strings blanked)
    ("struct", r"^\s*(pub\s+)?struct\s"),
    ("enum", r"^\s*(pub\s+)?enum\s"),
    ("match", r"\bmatch\b"),
    ("option", r"\bOption\[|\bSome\(|\bNone\b"),
    ("result", r"\bResult\[|\bOk\(|\bErr\("),
    ("try_op", r"\?"),
    ("comprehension", r"\[[^\[\]\n]*\bfor\b[^\[\]\n]*\bin\b"),
    ("inout", r"\binout\b"),
    ("contract", r"^\s*(requires|ensures)\b"),
    ("inline_test", r"^test\s"),
    ("module", r"^import\s"),
    ("fn_main", r"^fn main\("),
    ("cap_stdin", r"\bstdin\.read_line\b"),
    ("cap_args", r"\bargs\.list\b"),
    ("cap_fs_read", r"\.read\("),
    ("cap_fs_write", r"\.write\("),
    ("cap_fs_sub", r"\.sub\("),
    ("cap_env_vars", r"\bvars\.get\b"),
    ("while", r"^\s*while\b"),
    ("float", r"\bfloat\b|\d\.\d"),
]


def blank(src):
    src = re.sub(r'"(\\.|[^"\\])*"', '""', src)
    return re.sub(r"#.*", "", src)


def features(d):
    text = "\n".join(open(os.path.join(r, f)).read() for r, _, fs in os.walk(d)
                     for f in fs if f.endswith(".lim"))
    b = blank(text)
    return [n for n, rx in FEATURES if re.search(rx, b, re.M)]


_enc = None
def tokens(d, ext):
    global _enc
    if _enc is None:
        import tiktoken
        _enc = tiktoken.get_encoding("o200k_base")
    n = 0
    for r, _, fs in os.walk(d):
        if os.path.basename(r) == "tests":
            continue
        for f in fs:
            if f.endswith(ext):
                n += len(_enc.encode(open(os.path.join(r, f)).read()))
    return n


def run(cmd, case, cwd):
    for p, text in case.get("files", {}).items():
        fp = os.path.join(cwd, p)
        os.makedirs(os.path.dirname(fp), exist_ok=True)
        with open(fp, "w") as f:
            f.write(text)
    try:
        p = subprocess.run(cmd + case.get("args", []), input=case.get("stdin", ""), cwd=cwd, env=dict(os.environ, **case.get("env", {})),
                           capture_output=True, text=True, timeout=TIMEOUT)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return -1, "", "timeout"


def verify(name, write_expect=False):
    d = os.path.join(PAIRS, name)
    cases = json.load(open(os.path.join(d, "tests", "cases.json")))
    tmp = tempfile.mkdtemp(prefix="corpus-")
    try:
        exe = os.path.join(tmp, "prog.bin")
        b = subprocess.run([LIMITS, "build", "prog.lim", "-o", exe], cwd=d, capture_output=True, text=True)
        if b.returncode != 0:
            return "fail", "compile: " + (b.stdout + b.stderr).strip().splitlines()[0][:200] if (b.stdout + b.stderr).strip() else "compile failed"
        src = open(os.path.join(d, "prog.lim")).read()
        if re.search(r"^test\s", src, re.M):
            t = subprocess.run([LIMITS, "test", "prog.lim"], cwd=d, capture_output=True, text=True)
            if t.returncode != 0:
                return "fail", "inline tests: " + (t.stdout + t.stderr).strip()[-200:]
        for c in cases:
            outp = os.path.join(d, "tests", c["name"] + ".out")
            w = tempfile.mkdtemp(dir=tmp)
            shutil.copy(os.path.join(d, "prog.py"), w)
            for r, _, fs in os.walk(d):  # Python helper modules
                rel = os.path.relpath(r, d)
                if rel.startswith("tests"):
                    continue
                for f in fs:
                    if f.endswith(".py") and f != "prog.py":
                        os.makedirs(os.path.join(w, rel), exist_ok=True)
                        shutil.copy(os.path.join(r, f), os.path.join(w, rel, f))
            prc, pout, perr = run([sys.executable, "prog.py"], c, w)
            if prc != 0:
                return "fail", f"python exit {prc} on {c['name']}: {perr.strip()[-200:]}"
            if write_expect:
                with open(outp, "w") as f:
                    f.write(pout)
            want = open(outp).read()
            if pout != want:
                return "fail", f"python output differs from expected on {c['name']}"
            w2 = tempfile.mkdtemp(dir=tmp)
            lrc, lout, lerr = run([exe], c, w2)
            if lrc != 0:
                return "fail", f"limits exit {lrc} on {c['name']}: {lerr.strip()[-200:]}"
            if lout != want:
                return "fail", f"limits output differs on {c['name']}: got {lout[:80]!r} want {want[:80]!r}"
        return "pass", f"{len(cases)} cases"
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def main():
    args = sys.argv[1:]
    write = "--expect" in args
    args = [a for a in args if a != "--expect"]
    names = args or sorted(os.listdir(PAIRS))
    meta = {}
    mp = os.path.join(HERE, "sources.json")
    if os.path.exists(mp):
        meta = json.load(open(mp))
    rows, bad = [], 0
    for n in names:
        if write and n.startswith("bench_"):
            sys.exit("refusing --expect on bench pair " + n)
        status, why = verify(n, write)
        bad += status != "pass"
        print(f"{status:4} {n}: {why}", flush=True)
        d = os.path.join(PAIRS, n)
        rows.append({"name": n, "source": meta.get(n, "authored"), "features": features(d),
                     "tokens_py": tokens(d, ".py"), "tokens_lim": tokens(d, ".lim"),
                     "cases": len(json.load(open(os.path.join(d, "tests", "cases.json")))),
                     "status": status, **({} if status == "pass" else {"reason": why})})
    if not args:
        with open(os.path.join(HERE, "index.jsonl"), "w") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")
    print(f"{len(rows) - bad}/{len(rows)} pairs pass")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
