#!/usr/bin/env python3
"""Build the hard Limits-Bench task set (tasks-hard/) from DeepMind CodeContests.

Usage: python3 -I tools/build_hard_tasks.py [--jobs N]

Reads the parquet files in data/raw-hard/ (needs pyarrow), applies the selection
rule documented in HARD.md, verifies a dataset Python 3 solution on every kept
test (python3, 2 s per test) and writes tasks-hard/cc_<n>.json + cc_<n>.py and
data/selection_log_hard.txt. Deterministic: same raw files -> same tasks.
"""
import argparse, json, os, re, subprocess, sys, tempfile
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RAW = os.path.join(ROOT, "data", "raw-hard")
OUT = os.path.join(ROOT, "tasks-hard")
LOG = os.path.join(ROOT, "data", "selection_log_hard.txt")
FILES = [  # processed in this order
    "test-00000-of-00001-9c49eeff30aacaa8.parquet",
    "valid-00000-of-00001-5e672c5751f060d3.parquet",
    "train-00000-of-00039-e991a271dbfa9925.parquet",
]
RATING = (1200, 1900)
TARGET = 120
MAX_TESTS, MIN_TESTS = 30, 5
MAX_INPUT_BYTES = 100_000
PY_TIMEOUT = 2.0
MAX_REF_TRIES = 8
STATIC_ONLY = False  # --static-only: apply the filters that need no execution, write nothing
BAD_TAGS = {"interactive", "bitmasks", "hashing", "fft", "geometry", "probabilities",
            "*special", "string suffix structures", "matrices", "chinese remainder theorem"}
# statements whose answer is not unique, needs a float tolerance, or lost an image
BAD_TEXT = re.compile(r"any of them|multiple (possible )?(answers|solutions)|several (possible )?(answers|solutions)"
                      r"|print any|output any|any such|any one of|any valid|any possible|any correct"
                      r"|absolute or relative error|relative or absolute error|<image>|interaction", re.I)
# reference must avoid hash-based containers (Limits has no maps or sets)
HASHY = re.compile(r"\{|\bdict\b|\bset\(|\bfrozenset\b|Counter|defaultdict|OrderedDict|\.get\(")
INT_TOK = re.compile(r"^-?\d+$")
FLOAT_TOK = re.compile(r"^-?\d*\.\d+$")


def tokens(s):
    return s.split()


def out_ok(o):
    for t in o.split():
        if FLOAT_TOK.match(t):
            return "float in output"
        if INT_TOK.match(t) and not (-(2**63) <= int(t) < 2**63):
            return "integer outside 64 bits"
    return None


def run_ref(code, stdin):
    with tempfile.NamedTemporaryFile("w", suffix=".py", delete=False) as f:
        f.write(code)
        p = f.name
    try:
        r = subprocess.run([sys.executable, "-I", p], input=stdin, capture_output=True, text=True,
                           timeout=PY_TIMEOUT, errors="replace")
        return r.returncode, r.stdout
    except subprocess.TimeoutExpired:
        return None, ""
    finally:
        os.unlink(p)


def pick_tests(row):
    seen, tests = set(), []
    for origin in ("public", "private", "generated"):
        t = row[origin + "_tests"] or {"input": [], "output": []}
        for i, o in zip(t["input"], t["output"]):
            if i in seen or len(i.encode()) > MAX_INPUT_BYTES or not o.strip():
                continue
            seen.add(i)
            tests.append({"stdin": i if i.endswith("\n") else i + "\n", "stdout": o, "origin": origin})
            if len(tests) >= MAX_TESTS:
                return tests
    return tests


def consider(row):
    """Returns (reason_or_None, tests, ref_code)."""
    if row["source"] != 2:
        return "not Codeforces", None, None
    r = row["cf_rating"] or 0
    if not (RATING[0] <= r <= RATING[1]):
        return "rating %d outside %d-%d" % (r, *RATING), None, None
    bad = BAD_TAGS.intersection(row["cf_tags"] or [])
    if bad:
        return "tag " + ",".join(sorted(bad)), None, None
    if row["input_file"] or row["output_file"]:
        return "file I/O", None, None
    m = BAD_TEXT.search(row["description"])
    if m:
        return "statement: %r" % m.group(0), None, None
    tests = pick_tests(row)
    if len(tests) < MIN_TESTS:
        return "only %d usable tests" % len(tests), None, None
    for t in tests:
        why = out_ok(t["stdout"])
        if why:
            return why, None, None
    sols = row["solutions"] or {"language": [], "solution": []}
    cands = [s for l, s in zip(sols["language"], sols["solution"]) if l == 3 and not HASHY.search(s)]
    if not cands:
        return "no hash-free Python 3 solution in dataset", None, None
    if STATIC_ONLY:
        return None, tests, cands[0]
    for code in cands[:MAX_REF_TRIES]:
        ok = True
        for t in tests:
            rc, out = run_ref(code, t["stdin"])
            if rc != 0 or tokens(out) != tokens(t["stdout"]):
                ok = False
                break
        if ok:
            return None, tests, code
    return "no Python 3 solution passed all kept tests (tried %d)" % min(len(cands), MAX_REF_TRIES), None, None


CONTRACT = """

Write a complete program. It reads the input described above from standard input and prints the answer to standard output.
Output is compared token by token (whitespace-separated), so spacing and line breaks between tokens do not matter.
"""


def describe(row, tests):
    d = row["description"].strip()
    pub = [t for t in tests if t["origin"] == "public"][:2] or tests[:1]
    if "Example" not in d and "Input\n" not in d[-2000:]:
        for k, t in enumerate(pub, 1):
            d += "\n\nExample %d\nstdin:\n```\n%s```\nstdout:\n```\n%s\n```" % (k, t["stdin"], t["stdout"].rstrip("\n"))
    return d + CONTRACT


def main():
    import pyarrow.parquet as pq
    ap = argparse.ArgumentParser()
    ap.add_argument("--jobs", type=int, default=8)
    ap.add_argument("--static-only", action="store_true")
    a = ap.parse_args()
    global STATIC_ONLY
    STATIC_ONLY = a.static_only
    cols = ["name", "description", "public_tests", "private_tests", "generated_tests", "source",
            "solutions", "cf_contest_id", "cf_index", "cf_rating", "cf_tags", "input_file", "output_file"]
    rows = []
    for f in FILES:
        for r in pq.read_table(os.path.join(RAW, f), columns=cols).to_pylist():
            r["_file"] = f.split("-")[0]
            rows.append(r)
    with ThreadPoolExecutor(a.jobs) as ex:
        results = list(ex.map(consider, rows))
    if STATIC_ONLY:
        ok = [(r, t) for r, (w, t, c) in zip(rows, results) if w is None]
        for r, t in ok:
            print("candidate %s %s rating %d tests %d" % (r["_file"], r["name"], r["cf_rating"], len(t)))
        from collections import Counter
        print(Counter(w.split(":")[0].split(" (")[0] for w, _, _ in results if w))
        print("static candidates:", len(ok))
        return
    os.makedirs(OUT, exist_ok=True)
    for f in os.listdir(OUT):
        if f.startswith("cc_"):
            os.unlink(os.path.join(OUT, f))
    log, n = [], 0
    for row, (why, tests, code) in zip(rows, results):
        tag = "%s %s" % (row["_file"], row["name"])
        if why is None and n >= TARGET:
            why = "target of %d reached" % TARGET
        if why:
            if not why.startswith("not Codeforces") and not why.startswith("rating"):
                log.append("skip %s: %s" % (tag, why))
            continue
        n += 1
        tid = "cc_%03d" % n
        task = {"id": tid, "source": "CodeContests/%s/%s (Codeforces %s%s, rating %d)" % (
                    row["_file"], row["name"], row["cf_contest_id"], row["cf_index"], row["cf_rating"]),
                "description": describe(row, tests), "params": [], "output_type": "text",
                "compare": "tokens", "cf_rating": row["cf_rating"], "cf_tags": list(row["cf_tags"] or []),
                "tests": tests}
        with open(os.path.join(OUT, tid + ".json"), "w") as f:
            json.dump(task, f, indent=1)
        with open(os.path.join(OUT, tid + ".py"), "w") as f:
            f.write("# Reference: a Python 3 solution from the CodeContests dataset (%s), unchanged.\n" % row["name"]
                    + code)
        log.append("keep %s -> %s (%d tests)" % (tag, tid, len(tests)))
    with open(LOG, "w") as f:
        f.write("\n".join(log) + "\n")
    print("kept", n)


if __name__ == "__main__":
    main()
