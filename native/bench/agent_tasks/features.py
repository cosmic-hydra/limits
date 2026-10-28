#!/usr/bin/env python3
"""Token savings of hypothetical syntax rewrites. Usage: python3 features.py bench/agent_tasks"""
import re, glob, tiktoken, sys
e = tiktoken.get_encoding("o200k_base")
T = lambda s: len(e.encode(s))
def code(p):
    return "\n".join(l.rstrip() for l in open(p) if l.strip() and not l.strip().startswith("#")) + "\n"
files = sorted(glob.glob(sys.argv[1] + "/[0-9]*.lim"))
src = {f: code(f) for f in files}
base = sum(T(s) for s in src.values())
rw = {
 "compound assignment (x += e)": [(r"(?m)^(\s*)(\S+) = \2 ([-+*/%]) ", r"\1\2 \3= ")],
 "min/max builtins": [(r"(?m)^(\s*)if (\S+) ([<>]) (\S+) \{\n\s*\4 = \2;\n\s*\}", lambda m: f"{m[1]}{m[4]} = {'max' if m[3]=='>' else 'min'}({m[4]}, {m[2]});")],
 "list comprehension": [(r"(?m)^(\s*)let (\w+): \[[^\]]+\] = \[\];\n\s*for (\w+) in ([^{]+) \{\n\s*push\(\2, (.+)\);\n\s*\}", r"\1let \2 = [\5 for \3 in \4];")],
 "negative index xs[-1]": [(r"\[len\((\w+)\) - (\d+)", r"[-\2")],
 "indentation blocks (no braces)": [(r"(?m) \{$", ":"), (r"(?m)^\s*\}\n", "")],
 "untyped empty list (infer from use)": [(r": \[\w+\] = \[\]", " = []")],
 "no 'let' keyword": [(r"\blet ", "")],
 "module-level state (no seed param)": [(r"seed: \[int\], ", ""), (r"\(seed\)", "()"), (r"s: \[int\]", ""), (r"(?m)^\s*let seed = \[\d+\];\n", "")],
}
print("base", base)
for k, rules in rw.items():
    tot = 0; n = 0
    for f, s in src.items():
        t = s
        for a, b in rules:
            t, c = re.subn(a, b, t); n += c
        tot += T(s) - T(t)
    print(f"{k}: {n} sites, saves {tot} tokens ({100*tot/base:.1f}%)")
