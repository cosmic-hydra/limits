#!/usr/bin/env python3
"""Verify every agent task's Limits and Python outputs match, then measure
code size (non-blank non-comment lines, chars, LLM tokens) and run time.
Usage: python3 bench/agent_tasks/measure.py [--no-time]   (run from native/)"""
import glob, os, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
LIMITS = os.path.join(HERE, "../../target/release/limits")
try:
    import tiktoken
    ENCS = [tiktoken.get_encoding(n) for n in ("o200k_base", "cl100k_base")]
except ImportError:
    ENCS = []


def code(path):
    lines = [l.rstrip() for l in open(path)]
    return "\n".join(l for l in lines if l.strip() and not l.strip().startswith("#")) + "\n"


def size(path):
    c = code(path)
    return [c.count("\n"), len(c)] + [len(e.encode(c)) for e in ENCS]


def best(cmd):
    t = None
    for _ in range(3):
        t0 = time.perf_counter()
        r = subprocess.run(cmd, capture_output=True, text=True)
        dt = time.perf_counter() - t0
        t = dt if t is None else min(t, dt)
    return t, r.stdout


def build(lim, out):
    subprocess.run([LIMITS, "build", lim, "-o", out, "--max-steps", "100000000000"], check=True)


def main():
    timing = "--no-time" not in sys.argv
    subprocess.run(["cargo", "build", "--release", "-q"], cwd=os.path.join(HERE, "../.."), check=True)
    tmp = tempfile.mkdtemp()
    cols = ["lines", "chars"] + ["tok_o200k", "tok_cl100k"][: len(ENCS)]
    rows, ok = [], True
    for lim in sorted(glob.glob(os.path.join(HERE, "[0-9]*.lim"))):
        name = os.path.basename(lim)[:-4]
        py = lim[:-4] + ".py"
        exe = os.path.join(tmp, name)
        build(lim, exe)
        want = subprocess.run(["python3", py], capture_output=True, text=True).stdout
        got = subprocess.run([exe], capture_output=True, text=True).stdout
        if got != want:
            ok = False
            print(f"MISMATCH {name}:\n{got}\n---\n{want}", file=sys.stderr)
        before = os.path.join(HERE, "before", name + ".lim")
        bsize = None
        if os.path.exists(before):
            build(before, exe + ".b")
            if subprocess.run([exe + ".b"], capture_output=True, text=True).stdout != want:
                ok = False
                print(f"MISMATCH before/{name}", file=sys.stderr)
            bsize = size(before)
        elif not any(b in code(lim) for b in ("split(", "join(", "sort(")):
            bsize = size(lim)  # unchanged: does not use the new builtins
        tl = tp = None
        if timing:
            tl, _ = best([exe])
            tp, _ = best(["python3", py])
        rows.append((name, size(lim), size(py), bsize, tl, tp))
    print("| task | " + " | ".join(f"Limits {c}" for c in cols) + " | " + " | ".join(f"Py {c}" for c in cols)
          + " | before tok_o200k" + " | Limits s | Python s | speedup |")
    print("|" + "---|" * (2 * len(cols) + 5))
    tot = [[0] * len(cols) for _ in range(2)]
    for name, l, p, b, tl, tp in rows:
        for i in range(len(cols)):
            tot[0][i] += l[i]
            tot[1][i] += p[i]
        t = f"{tl:.3f} | {tp:.3f} | {tp / tl:.1f}x" if timing else "- | - | -"
        print(f"| {name} | " + " | ".join(map(str, l)) + " | " + " | ".join(map(str, p))
              + f" | {b[2] if b else 'n/a'} | {t} |")
    tt = f"{sum(r[4] for r in rows):.3f} | {sum(r[5] for r in rows):.3f} | {sum(r[5] for r in rows) / sum(r[4] for r in rows):.1f}x" if timing else "- | - | -"
    print("| **total** | " + " | ".join(map(str, tot[0])) + " | " + " | ".join(map(str, tot[1])) + f" | | {tt} |")
    exp = [r for r in rows if r[3]]
    print(f"\nexpressible before the new builtins: {len(exp)}/20; on those, tok_o200k: before "
          f"{sum(r[3][2] for r in exp)}, after {sum(r[1][2] for r in exp)}, Python {sum(r[2][2] for r in exp)}")
    if not ENCS:
        print("tiktoken unavailable: token columns omitted")
    print("all outputs match" if ok else "OUTPUT MISMATCH")
    sys.exit(0 if ok else 1)


main()
