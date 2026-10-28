#!/usr/bin/env python3
"""Run `limits check --stdin --expect --json --time` on prefixes of programs
that check clean, and report:
  - prefixes for which --partial reported an error (should be none: every
    prefix of a valid program is a valid unfinished program),
  - p50/p95/max of the in-process time (check + expect, from --time) and of
    the wall-clock time per call (includes process start).

usage: bench/partial_prefixes.py [--step N] [--show] FILE.lim...
Run from native/ after `cargo build --release`. Each prefix is checked from
the file's own directory so `import`s resolve.
"""
import json
import os
import subprocess
import sys
import time

LIMITS = os.path.abspath("target/release/limits")


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(p / 100 * (len(xs) - 1))))]


def main():
    args = sys.argv[1:]
    step, show = 1, False
    files = []
    while args:
        a = args.pop(0)
        if a == "--step":
            step = int(args.pop(0))
        elif a == "--show":
            show = True
        else:
            files.append(a)
    inproc, wall, bad, n = [], [], [], 0
    for f in files:
        src = open(f, encoding="utf-8").read()
        d = os.path.dirname(os.path.abspath(f))
        full = subprocess.run([LIMITS, "check", "--json", os.path.abspath(f)], capture_output=True)
        # Modules have no main (E0213); that is clean too.
        codes = [json.loads(l)["code"] for l in full.stderr.decode().splitlines()]
        if any(c != "E0213" for c in codes):
            print(f"skip {f}: does not check clean")
            continue
        cuts = sorted(set(list(range(0, len(src) + 1, step)) + [len(src)]))
        for k in cuts:
            pre = src[:k]
            t0 = time.perf_counter()
            r = subprocess.run(
                [LIMITS, "check", "--stdin", "--expect", "--json", "--time"],
                input=pre.encode(), capture_output=True, cwd=d)
            wall.append((time.perf_counter() - t0) * 1e3)
            n += 1
            lines = r.stdout.decode().splitlines()
            diags = [json.loads(l) for l in lines if '"kind":"expect"' not in l]
            exp = [l for l in lines if '"kind":"expect"' in l]
            if diags or len(exp) != 1:
                bad.append((f, k, diags[:1]))
            for l in r.stderr.decode().splitlines():
                if l.startswith("check: "):
                    a, b = l[7:].split(" ms, expect: ")
                    inproc.append(float(a) + float(b.split(" ")[0]))
    print(f"prefixes: {n}, with errors reported: {len(bad)}")
    seen = set()
    for f, k, d in bad:
        key = (f, d[0]["msg"] if d else "no expect")
        if key in seen and not show:
            continue
        seen.add(key)
        print(f"  {f} @ {k}: {d[0]['line'] if d else ''}: {key[1]}")
    if inproc:
        print("in-process ms (check+expect): p50 %.3f  p95 %.3f  max %.3f" % (
            pct(inproc, 50), pct(inproc, 95), max(inproc)))
        print("wall ms per call (incl. process start): p50 %.3f  p95 %.3f  max %.3f" % (
            pct(wall, 50), pct(wall, 95), max(wall)))


if __name__ == "__main__":
    main()
