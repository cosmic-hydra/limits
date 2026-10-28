#!/usr/bin/env python3
"""Build language-neutral Limits-Bench tasks from the raw HumanEval / MBPP files.

Usage: python3 -I tools/build_tasks.py            (from anywhere; paths are fixed)

Writes tasks/<id>.json (description, I/O contract, tests) and tasks/<id>.py
(the official reference solution wrapped in the stdin/stdout contract).
Deterministic: same raw files -> same tasks (fixed RNG seed, dataset order).
See README.md "Selection rule" for what is kept and why.
"""
import ast, gzip, json, os, random, re, subprocess, sys, textwrap

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RAW = os.path.join(ROOT, "data", "raw")
OUT = os.path.join(ROOT, "tasks")
PER_SOURCE = 30
MIN_TESTS = 5
INT_MIN, INT_MAX = -(2**63), 2**63 - 1
SEED = 20261028

# Manual exclusions, applied in dataset order before counting. Every entry has a reason.
EXCLUDE = {
    # HumanEval
    "HumanEval/38": "input is produced by an encoder in the prompt (cyclic encode/decode pair)",
    "HumanEval/50": "input is produced by an encoder in the prompt (shift encode/decode pair)",
    "HumanEval/32": "float coefficients and tolerance-based check",
    "HumanEval/162": "needs an MD5 implementation",
    "HumanEval/53": "tests are randomised in a loop (add); trivial anyway",
    # MBPP: answers where the official test accepts several outputs are filtered automatically
}

TYPE_WORDS = {
    "int": "an integer",
    "bool": "a boolean",
    "str": "a string",
    "[int]": "a list of integers",
    "[str]": "a list of strings",
    "[[int]]": "a list of lists of integers",
}
ENC_WORDS = {
    "int": "one line holding the integer in decimal (e.g. `-12`)",
    "bool": "one line, `true` or `false`",
    "str": "one line holding the string exactly (it may be empty or contain spaces; it never contains a newline)",
    "[int]": "one line with the integers separated by single spaces (an empty line for an empty list)",
    "[str]": "one line with the count n, then n lines, one string per line",
    "[[int]]": "one line with the count n, then n lines, each a list of integers separated by single spaces (an empty line for an empty inner list)",
}


# Hand-written extra inputs for tasks whose inputs have structure that random
# generation would break (balanced brackets, equal-length bit strings, ...).
# Used instead of random augmentation; expected outputs still come from the reference.
HAND_INPUTS = {
    "HumanEval/1": [("()",), ("(()) ((())) () ((())()())",), ("(()(())) () (())",)],
    "HumanEval/6": [("()",), ("((())) (()) ()",), ("(()()) ()()()",)],
    "HumanEval/11": [("0", "1"), ("1111", "1010"), ("100", "001")],
    "MBPP/74": [(["a", "b", "a"], ["x", "y", "x"]), (["a", "a"], ["x", "y"]), (["a", "b"], ["x", "x"])],
}

# ---------------------------------------------------------------- types
def typeof(v):
    if isinstance(v, bool):
        return "bool"
    if isinstance(v, int):
        return "int" if INT_MIN <= v <= INT_MAX else None
    if isinstance(v, str):
        return "str" if "\n" not in v and "\r" not in v else None
    if isinstance(v, list):
        ts = {typeof(x) for x in v}
        if not v:
            return "[?]"
        if None in ts:
            return None
        if ts == {"[?]"}:
            return None
        ts.discard("[?]") if ts - {"[?]"} == {"[int]"} else None
        if len(ts) != 1:
            return None
        t = ts.pop()
        if t in ("int", "str"):
            return "[" + t + "]"
        if t == "[int]":
            return "[[int]]"
        return None
    return None


def unify(a, b):
    if a == b:
        return a
    if a == "[?]" and b and b.startswith("["):
        return b
    if b == "[?]" and a and a.startswith("["):
        return a
    return None


def encode(v, t):
    if t == "int":
        return [str(v)]
    if t == "bool":
        return ["true" if v else "false"]
    if t == "str":
        return [v]
    if t == "[int]":
        return [" ".join(str(x) for x in v)]
    if t == "[str]":
        return [str(len(v))] + list(v)
    if t == "[[int]]":
        return [str(len(v))] + [" ".join(str(x) for x in r) for r in v]
    raise ValueError(t)


def fits(v, t):
    if t == "[?]":
        return False
    tt = typeof(v)
    return tt == t or (tt == "[?]" and t.startswith("["))


# ---------------------------------------------------------------- python wrapper
DECODER = r'''
import sys as _sys
_lines = _sys.stdin.read().split("\n")
_pos = 0
def _line():
    global _pos
    s = _lines[_pos] if _pos < len(_lines) else ""
    _pos += 1
    return s
def _ints(s):
    return [int(x) for x in s.split(" ")] if s != "" else []
def _read(t):
    if t == "int": return int(_line())
    if t == "bool": return _line() == "true"
    if t == "str": return _line()
    if t == "[int]": return _ints(_line())
    if t == "[str]":
        n = int(_line()); return [_line() for _ in range(n)]
    if t == "[[int]]":
        n = int(_line()); return [_ints(_line()) for _ in range(n)]
    raise ValueError(t)
def _write(v, t):
    if t == "bool": print("true" if v else "false")
    elif t in ("int", "str"): print(v)
    elif t == "[int]": print(" ".join(str(x) for x in v))
    elif t == "[str]":
        print(len(v))
        for x in v: print(x)
    elif t == "[[int]]":
        print(len(v))
        for r in v: print(" ".join(str(x) for x in r))
    else: raise ValueError(t)
'''


def wrapper(ref_code, func, in_types, out_type):
    return (
        "# Reference solution (official, unchanged) wrapped in the Limits-Bench I/O contract.\n"
        + ref_code.rstrip()
        + "\n\n"
        + DECODER
        + "\nif __name__ == '__main__':\n"
        + f"    _args = [_read(t) for t in {in_types!r}]\n"
        + f"    _write({func}(*_args), {out_type!r})\n"
    )


def run_ref(ref_code, func, args, timeout=5):
    """Call the reference on literal args in a fresh python3 -I; returns (ok, value)."""
    prog = ref_code + "\nimport sys, json\n" + f"_r = {func}(*{args!r})\nprint(repr(_r))\n"
    try:
        p = subprocess.run([sys.executable, "-I", "-c", prog], capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return False, None
    if p.returncode != 0:
        return False, None
    try:
        return True, ast.literal_eval(p.stdout.strip())
    except Exception:
        return False, None


# ---------------------------------------------------------------- test extraction
def extract_calls(test_src, fname):
    """Arg tuples from `assert f(..) == lit`, `assert f(..)`, `assert not f(..)`, `assert f(..) is X`.
    Returns (calls, ok) where ok=False if some assert calls f in another shape (e.g. set(f(..)))."""
    calls, ok = [], True
    try:
        tree = ast.parse(test_src)
    except SyntaxError:
        return [], False
    for node in ast.walk(tree):
        if not isinstance(node, ast.Assert):
            continue
        e = node.test
        if isinstance(e, ast.UnaryOp) and isinstance(e.op, ast.Not):
            e = e.operand
        if isinstance(e, ast.Compare) and len(e.ops) == 1 and isinstance(e.ops[0], (ast.Eq, ast.Is)):
            e = e.left
        if isinstance(e, ast.Call) and isinstance(e.func, ast.Name) and e.func.id == fname and not e.keywords:
            try:
                calls.append(tuple(ast.literal_eval(a) for a in e.args))
            except Exception:
                ok = False
        else:
            # some other assert shape mentioning the function: answer may not be unique
            if any(isinstance(n, ast.Name) and n.id == fname for n in ast.walk(node)):
                ok = False
    return calls, ok


def param_names(code, fname):
    for node in ast.walk(ast.parse(code)):
        if isinstance(node, ast.FunctionDef) and node.name == fname:
            return [a.arg for a in node.args.args], ast.get_docstring(node)
    return None, None


# ---------------------------------------------------------------- augmentation
def gen_like(rng, t, seen):
    """Random value of type t shaped like the observed values `seen`."""
    if t == "int":
        lo, hi = min(seen), max(seen)
        return rng.randint(lo, hi)
    if t == "bool":
        return rng.random() < 0.5
    if t == "str":
        chars = sorted(set("".join(seen))) or ["a"]
        lens = [len(s) for s in seen]
        return "".join(rng.choice(chars) for _ in range(rng.randint(min(lens), max(lens))))
    if t in ("[int]", "[str]"):
        el = [x for s in seen for x in s]
        lens = [len(s) for s in seen]
        n = rng.randint(min(lens), max(lens))
        if not el:
            return []
        return [gen_like(rng, t[1:-1], el) for _ in range(n)]
    if t == "[[int]]":
        lens = [len(s) for s in seen]
        rows = [r for s in seen for r in s]
        n = rng.randint(min(lens), max(lens))
        return [gen_like(rng, "[int]", rows) if rows else [] for _ in range(n)]
    raise ValueError(t)


# ---------------------------------------------------------------- descriptions
def clean_docstring(doc):
    """Drop doctest examples (`>>>` lines and their outputs) and Python-only lines."""
    out, skip = [], False
    for ln in doc.split("\n"):
        s = ln.strip()
        if s.startswith(">>>"):
            skip = True
            continue
        if skip:
            if s == "":
                skip = False
            continue
        out.append(ln)
    text = textwrap.dedent("\n".join(out)).strip()
    text = re.sub(r"\n(?:For )?[Ee]xamples?:?\s*$", "", text).strip()
    return text


def contract_text(params, in_types, out_type):
    lines = ["Input (stdin), one value after another:"]
    for p, t in zip(params, in_types):
        lines.append(f"- `{p}`: {TYPE_WORDS[t]}, written as {ENC_WORDS[t]}.")
    lines.append("")
    lines.append(f"Output (stdout): the result, {TYPE_WORDS[out_type]}, written as {ENC_WORDS[out_type]}.")
    if out_type == "[int]":
        lines.append("Print an empty line for an empty list.")
    return "\n".join(lines)


def example_text(tests, k=2):
    parts = []
    for i, tc in enumerate(tests[:k]):
        parts.append(f"Example {i + 1}\nstdin:\n```\n{tc['stdin']}```\nstdout:\n```\n{tc['stdout']}```")
    return "\n\n".join(parts)


# ---------------------------------------------------------------- main build
def build_one(src, tid, code, fname, test_src, docstring_text, rng):
    params, doc = param_names(code, fname)
    if params is None:
        return None, "entry function not found"
    calls, shape_ok = extract_calls(test_src, fname)
    if not shape_ok:
        return None, "a test checks the result in a non-equality form (e.g. set(...), tolerance) or with non-literal args"
    if not calls:
        return None, "no literal test calls"
    if any(len(c) != len(params) for c in calls):
        return None, "default/variadic arguments used"
    # infer input types
    in_types = []
    for i in range(len(params)):
        t = typeof(calls[0][i])
        for c in calls[1:]:
            t = unify(t, typeof(c[i])) if t else None
        if t in (None, "[?]"):
            return None, f"parameter {params[i]} has an unsupported type"
        in_types.append(t)
    # official expected outputs, recomputed by the reference
    results = []
    for c in calls:
        ok, v = run_ref(code, fname, list(c))
        if not ok:
            return None, "reference fails on an official input"
        results.append((c, v))
    out_type = None
    for _, v in results:
        t = typeof(v)
        out_type = t if out_type is None else unify(out_type, t)
        if out_type is None:
            return None, "return type unsupported (float, tuple, dict, None, mixed...)"
    if out_type == "[?]":
        out_type = "[int]"
    if out_type not in TYPE_WORDS:
        return None, "return type unsupported"
    # dedupe official
    seen_keys, tests = set(), []
    for c, v in results:
        k = repr(c)
        if k in seen_keys:
            continue
        seen_keys.add(k)
        tests.append((c, v, "official"))
    for c in HAND_INPUTS.get(src, []):
        ok, v = run_ref(code, fname, list(c))
        assert ok and repr(c) not in seen_keys, (src, c)
        seen_keys.add(repr(c))
        tests.append((c, v, "hand"))
    # augment up to MIN_TESTS with random inputs shaped like the official ones
    tries = 0 if src not in HAND_INPUTS else 10**9
    while len(tests) < MIN_TESTS and tries < 200:
        tries += 1
        c = tuple(gen_like(rng, t, [x[i] for x, _, _ in tests]) for i, t in enumerate(in_types))
        if repr(c) in seen_keys:
            continue
        if not all(fits(c[i], t) or (typeof(c[i]) == t) for i, t in enumerate(in_types)):
            continue
        ok, v = run_ref(code, fname, list(c))
        if not ok or not (fits(v, out_type) or typeof(v) == out_type):
            continue
        seen_keys.add(repr(c))
        tests.append((c, v, "generated"))
    if len(tests) < MIN_TESTS:
        return None, "could not reach 5 tests"
    tcs = []
    for c, v, origin in tests:
        stdin = "\n".join(l for i, t in enumerate(in_types) for l in encode(c[i], t)) + "\n"
        stdout = "\n".join(encode(v, out_type)) + "\n"
        tcs.append({"stdin": stdin, "stdout": stdout, "origin": origin})
    desc = docstring_text
    contract = contract_text(params, in_types, out_type)
    full = (
        desc.strip()
        + "\n\nWrite a complete program. It reads its input from standard input and prints the answer to standard output.\n\n"
        + contract
        + "\n\n"
        + example_text(tcs, k=2 if src.startswith("HumanEval") else sum(t["origin"] == "official" for t in tcs))
    )
    task = {
        "id": tid,
        "source": src,
        "description": full,
        "params": [{"name": p, "type": t} for p, t in zip(params, in_types)],
        "output_type": out_type,
        "tests": tcs,
    }
    return (task, wrapper(code, fname, in_types, out_type)), None


def main():
    rng = random.Random(SEED)
    os.makedirs(OUT, exist_ok=True)
    for f in os.listdir(OUT):
        if f.endswith((".json", ".py")):
            os.remove(os.path.join(OUT, f))
    log = []
    he = [json.loads(l) for l in gzip.open(os.path.join(RAW, "HumanEval.jsonl.gz"))]
    mb = json.load(open(os.path.join(RAW, "sanitized-mbpp.json")))
    chosen = []

    n = 0
    for p in he:
        if n >= PER_SOURCE:
            break
        src = p["task_id"]
        if src in EXCLUDE:
            log.append((src, "excluded: " + EXCLUDE[src]))
            continue
        code = p["prompt"] + p["canonical_solution"]
        fname = p["entry_point"]
        _, doc = param_names(code, fname)
        test_src = p["test"].replace("candidate", fname)
        desc = clean_docstring(doc or "")
        desc = re.sub(r"\bTrue\b", "true", re.sub(r"\bFalse\b", "false", desc))
        desc = re.sub(r"\bfunction\b", "program", desc)
        tid = "he_%03d" % int(src.split("/")[1])
        r, why = build_one(src, tid, code, fname, test_src, desc, rng)
        if r is None:
            log.append((src, "skipped: " + why))
            continue
        chosen.append(r)
        log.append((src, "kept as " + tid))
        n += 1

    n = 0
    for p in mb:
        if n >= PER_SOURCE:
            break
        src = "MBPP/%d" % p["task_id"]
        if src in EXCLUDE:
            log.append((src, "excluded: " + EXCLUDE[src]))
            continue
        code = "\n".join(p["test_imports"]) + "\n" + p["code"]
        test_src = "\n".join(p["test_list"])
        m = re.search(r"assert\s+(?:not\s+)?(?:\w+\()*?([A-Za-z_]\w*)\(", p["test_list"][0])
        fname = None
        for node in ast.walk(ast.parse(p["code"])):
            if isinstance(node, ast.FunctionDef) and re.search(r"\b%s\(" % node.name, test_src):
                fname = node.name
                break
        if fname is None:
            log.append((src, "skipped: entry function not found"))
            continue
        desc = re.sub(r"\bfunction\b", "program", p["prompt"].strip())
        desc = re.sub(r"\bpython program\b", "program", desc, flags=re.I)
        tid = "mbpp_%03d" % p["task_id"]
        r, why = build_one(src, tid, code, fname, test_src, desc, rng)
        if r is None:
            log.append((src, "skipped: " + why))
            continue
        chosen.append(r)
        log.append((src, "kept as " + tid))
        n += 1

    for task, py in chosen:
        with open(os.path.join(OUT, task["id"] + ".json"), "w") as f:
            json.dump(task, f, indent=1)
            f.write("\n")
        with open(os.path.join(OUT, task["id"] + ".py"), "w") as f:
            f.write(py)
    with open(os.path.join(ROOT, "data", "selection_log.txt"), "w") as f:
        for s, why in log:
            f.write(f"{s}\t{why}\n")
    print(f"{len(chosen)} tasks written to {OUT}")


if __name__ == "__main__":
    main()
