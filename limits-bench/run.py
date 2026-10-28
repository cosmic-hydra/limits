#!/usr/bin/env python3
"""Limits-Bench harness: grade a directory of solutions against tasks/<id>.json.

  python3 run.py SOLDIR [--tasks DIR] [--lang limits|python|auto]
                 [--limits-bin PATH] [--cc PATH] [--timeout SEC] [--build-timeout SEC]
                 [--only ID,ID] [--out DIR] [--repair-log]

Plain mode: SOLDIR holds <id>.lim or <id>.py (one solution per task).
--repair-log: SOLDIR holds <id>.attempt1.lim, <id>.attempt2.lim, <id>.attempt3.lim
(or .py); every attempt present is graded and per-round repair success is reported.

Writes OUT/results.json, OUT/summary.md and OUT/feedback/<id>[.attemptN].txt
(the diagnostics an agent may see before its next repair). stdlib only;
uses tiktoken (cl100k_base) for token counts when importable, else characters.
"""
import argparse, json, os, shutil, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_BIN = os.path.join(os.path.dirname(HERE), "native", "target", "release", "limits")
STATUSES = ["pass", "compile_error", "runtime_error", "timeout", "wrong_answer", "missing", "toolchain_error"]

try:
    import tiktoken

    _ENC = tiktoken.get_encoding("cl100k_base")
    TOKEN_UNIT = "tiktoken cl100k_base tokens"

    def count_tokens(s):
        return len(_ENC.encode(s))
except Exception:  # pragma: no cover
    TOKEN_UNIT = "characters (tiktoken unavailable)"

    def count_tokens(s):
        return len(s)


def norm(s):
    """Output comparison: CRLF -> LF, trailing newlines ignored; everything else exact."""
    return s.replace("\r\n", "\n").rstrip("\n")


def same_output(out, exp, mode="exact"):
    """mode "exact" (default, tasks/): norm() equality; "tokens" (tasks-hard/): whitespace-split tokens equal."""
    if mode == "tokens":
        return out.split() == exp.split()
    return norm(out) == norm(exp)


def load_tasks(d, only):
    tasks = {}
    for f in sorted(os.listdir(d)):
        if f.endswith(".json"):
            t = json.load(open(os.path.join(d, f)))
            if not only or t["id"] in only:
                tasks[t["id"]] = t
    return tasks


def run_proc(cmd, stdin, timeout, env=None):
    t0 = time.monotonic()
    try:
        p = subprocess.run(cmd, input=stdin, capture_output=True, text=True, timeout=timeout, env=env,
                           errors="replace")
        return p.returncode, p.stdout, p.stderr, time.monotonic() - t0, False
    except subprocess.TimeoutExpired as e:
        out = e.stdout.decode(errors="replace") if isinstance(e.stdout, bytes) else (e.stdout or "")
        return None, out, "", time.monotonic() - t0, True


def parse_diags(stderr):
    diags = []
    for ln in stderr.splitlines():
        ln = ln.strip()
        if ln.startswith("{"):
            try:
                diags.append(json.loads(ln))
            except ValueError:
                pass
    return diags


def build_limits(src, args, workdir):
    """Returns (status_or_None, exe, diagnostics, stderr)."""
    exe = os.path.join(workdir, "prog")
    env = dict(os.environ)
    if args.cc:
        env["CC"] = args.cc
    rc, out, err, _, to = run_proc([args.limits_bin, "build", src, "-o", exe, "--json"], None,
                                   args.build_timeout, env)
    if to:
        return "toolchain_error", None, [], "build timed out"
    diags = parse_diags(err)
    if rc == 0:
        return None, exe, diags, err
    if rc == 1 and any(d.get("kind") in ("lex", "parse", "check") for d in diags):
        return "compile_error", None, diags, err
    return "toolchain_error", None, diags, err


def grade(task, src, lang, args):
    code = open(src, encoding="utf-8", errors="replace").read()
    res = {"file": os.path.relpath(src), "lang": lang, "tokens": count_tokens(code),
           "chars": len(code), "tests": [], "diagnostics": [], "feedback": ""}
    work = tempfile.mkdtemp(prefix="lbench-")
    try:
        if lang == "limits":
            st, exe, diags, err = build_limits(src, args, work)
            res["diagnostics"] = diags
            if st:
                res["status"] = st
                res["first_error"] = diags[0] if diags else {"msg": err.strip()[:500]}
                res["feedback"] = err.strip()
                return res
            cmd = [exe]
        else:
            cmd = [sys.executable, "-I", src]
        worst = "pass"
        order = {"pass": 0, "wrong_answer": 1, "timeout": 2, "runtime_error": 3}
        for i, tc in enumerate(task["tests"]):
            rc, out, err, dt, to = run_proc(cmd, tc["stdin"], args.timeout)
            if to:
                st = "timeout"
            elif rc != 0:
                st = "runtime_error"
            elif not same_output(out, tc["stdout"], task.get("compare", "exact")):
                st = "wrong_answer"
            else:
                st = "pass"
            res["tests"].append({"i": i, "status": st, "secs": round(dt, 4)})
            if st != "pass" and not res["feedback"]:
                if lang == "limits":
                    rd = parse_diags(err)
                    res["diagnostics"] += rd
                    if rd:
                        res["first_error"] = rd[0]
                res["feedback"] = (
                    f"test {i}: {st}\n--- stdin\n{tc['stdin']}--- expected stdout\n{tc['stdout']}"
                    f"--- actual stdout\n{out[:2000]}\n--- stderr\n{err.strip()[:2000]}"
                    + ("\n(timed out after %ss)" % args.timeout if to else "")
                )
            if order[st] > order[worst]:
                worst = st
        res["status"] = worst
        return res
    finally:
        shutil.rmtree(work, ignore_errors=True)


def find_solution(soldir, tid, lang, suffix=""):
    exts = {"limits": [".lim"], "python": [".py"], "auto": [".lim", ".py"]}[lang]
    for e in exts:
        p = os.path.join(soldir, tid + suffix + e)
        if os.path.exists(p):
            return p, ("limits" if e == ".lim" else "python")
    return None, None


def rate(rs, st):
    return sum(r["status"] == st for r in rs) / len(rs) if rs else 0.0


def summarize(per_task, final_key):
    rs = [per_task[t][final_key] for t in per_task if per_task[t].get(final_key)]
    n = len(per_task)
    passing = [r for r in rs if r["status"] == "pass"]
    s = {"tasks": n, "token_unit": TOKEN_UNIT}
    for st in STATUSES:
        s[st] = sum(r["status"] == st for r in rs) + (n - len(rs) if st == "missing" else 0)
    s["pass@1"] = s["pass"] / n if n else 0.0
    for st in ("compile_error", "runtime_error", "timeout", "wrong_answer"):
        s[st + "_rate"] = s[st] / n if n else 0.0
    s["mean_tokens_passing"] = (sum(r["tokens"] for r in passing) / len(passing)) if passing else None
    s["mean_tokens_all"] = (sum(r["tokens"] for r in rs) / len(rs)) if rs else None
    return s


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("soldir")
    ap.add_argument("--tasks", default=os.path.join(HERE, "tasks"))
    ap.add_argument("--lang", default="auto", choices=["auto", "limits", "python"])
    ap.add_argument("--limits-bin", default=DEFAULT_BIN)
    ap.add_argument("--cc", default=None, help="C compiler passed to limits as $CC (e.g. a tcc binary)")
    ap.add_argument("--timeout", type=float, default=10.0, help="seconds per test run")
    ap.add_argument("--build-timeout", type=float, default=120.0)
    ap.add_argument("--only", default="", help="comma-separated task ids")
    ap.add_argument("--out", default=None, help="output dir (default SOLDIR/_results)")
    ap.add_argument("--repair-log", action="store_true", help="grade <id>.attemptN.* files, N=1..3")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()
    only = set(x for x in args.only.split(",") if x)
    if not os.path.isdir(args.tasks) and os.path.isdir(os.path.join(HERE, args.tasks)):
        args.tasks = os.path.join(HERE, args.tasks)  # e.g. --tasks tasks-hard from any cwd
    tasks = load_tasks(args.tasks, only)
    if not tasks:
        sys.exit("no tasks found in " + args.tasks)
    out = args.out or os.path.join(args.soldir, "_results")
    os.makedirs(os.path.join(out, "feedback"), exist_ok=True)

    per_task = {}
    rounds = [1, 2, 3] if args.repair_log else [None]
    for tid, task in tasks.items():
        entry = {"attempts": []}
        for k in rounds:
            suffix = "" if k is None else ".attempt%d" % k
            src, lang = find_solution(args.soldir, tid, args.lang, suffix)
            if not src:
                if k is None or k == 1:
                    entry["attempts"].append({"attempt": k or 1, "status": "missing", "tokens": 0})
                break
            r = grade(task, src, lang, args)
            r["attempt"] = k or 1
            entry["attempts"].append(r)
            with open(os.path.join(out, "feedback", tid + suffix + ".txt"), "w") as f:
                f.write(r["status"] + "\n" + r["feedback"] + "\n")
            if r["status"] == "pass":
                break  # later attempts are not needed (and ignored if present)
        entry["first"] = entry["attempts"][0]
        entry["final"] = entry["attempts"][-1]
        entry["status"] = entry["first"]["status"]
        entry["final_status"] = entry["final"]["status"]
        entry["solved_at_attempt"] = next((a["attempt"] for a in entry["attempts"] if a["status"] == "pass"), None)
        per_task[tid] = entry
        if not args.quiet:
            print(f"{tid:10s} " + " -> ".join(a["status"] for a in entry["attempts"]), flush=True)

    summary = {"first_attempt": summarize(per_task, "first")}
    if args.repair_log:
        summary["final"] = summarize(per_task, "final")
        rep = []
        for k in (2, 3):
            failing_before = [t for t, e in per_task.items()
                              if e["solved_at_attempt"] is None or e["solved_at_attempt"] >= k]
            tried = [t for t in failing_before if len(per_task[t]["attempts"]) >= k]
            fixed = [t for t in tried if per_task[t]["solved_at_attempt"] == k]
            rep.append({"round": k, "failing_before": len(failing_before), "repair_submitted": len(tried),
                        "fixed": len(fixed),
                        "repair_success": (len(fixed) / len(tried)) if tried else None})
        summary["repair_rounds"] = rep
        n = len(per_task)
        summary["pass@attempt"] = {k: sum(1 for e in per_task.values() if e["solved_at_attempt"]
                                          and e["solved_at_attempt"] <= k) / n for k in (1, 2, 3)}
    res = {"soldir": os.path.abspath(args.soldir), "limits_bin": args.limits_bin, "cc": args.cc,
           "timeout": args.timeout, "summary": summary, "tasks": per_task}
    with open(os.path.join(out, "results.json"), "w") as f:
        json.dump(res, f, indent=1)

    md = render_md(summary, per_task, args.repair_log)
    with open(os.path.join(out, "summary.md"), "w") as f:
        f.write(md)
    print(md)
    print("wrote", os.path.join(out, "results.json"))


def pct(x):
    return "-" if x is None else "%.1f%%" % (100 * x)


def render_md(summary, per_task, repair):
    s = summary["first_attempt"]
    L = ["| metric | value |", "|---|---|",
         f"| tasks | {s['tasks']} |",
         f"| pass@1 | {pct(s['pass@1'])} ({s['pass']}) |",
         f"| compile-error rate (lex/parse/check) | {pct(s['compile_error_rate'])} ({s['compile_error']}) |",
         f"| runtime-error rate | {pct(s['runtime_error_rate'])} ({s['runtime_error']}) |",
         f"| timeout rate | {pct(s['timeout_rate'])} ({s['timeout']}) |",
         f"| wrong-answer rate | {pct(s['wrong_answer_rate'])} ({s['wrong_answer']}) |",
         f"| missing / toolchain error | {s['missing']} / {s['toolchain_error']} |",
         f"| mean size of passing solutions | "
         f"{'-' if s['mean_tokens_passing'] is None else '%.1f' % s['mean_tokens_passing']} ({s['token_unit']}) |"]
    if repair:
        f = summary["final"]
        L.append(f"| pass after repairs | {pct(f['pass@1'])} ({f['pass']}) |")
        for r in summary["repair_rounds"]:
            L.append(f"| repair round {r['round']} success | {pct(r['repair_success'])} "
                     f"({r['fixed']}/{r['repair_submitted']}) |")
    L += ["", "| task | attempts | first error |", "|---|---|---|"]
    for t, e in per_task.items():
        fe = e["first"].get("first_error") or {}
        msg = (fe.get("code", "") + " " + fe.get("msg", "")).strip().replace("|", "\\|")[:80]
        L.append(f"| {t} | {' -> '.join(a['status'] for a in e['attempts'])} | {msg} |")
    return "\n".join(L) + "\n"


if __name__ == "__main__":
    main()
