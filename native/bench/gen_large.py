#!/usr/bin/env python3
"""Generate a synthetic limits program (17 lines per function) for
compile-time measurement.
Usage: gen_large.py [n_funcs] > large.lim"""
import sys

n = int(sys.argv[1]) if len(sys.argv) > 1 else 1000
out = ["struct P:", "    x: int", "    y: float", "    name: str", "    xs: [int]", ""]
for i in range(n):
    out += [
        f"fn f{i}(a: int, p: inout P) -> int:",
        f"    var s = a + {i}",
        "    for k in 0..a:",
        "        if k % 3 == 0:",
        "            s += k * 2",
        "        elif k % 3 == 1:",
        "            s -= 1",
        "        else:",
        "            push(p.xs, k)",
        "    while s > 1000:",
        "        s /= 2",
        "    p.y += float(s) * 0.5",
        f'    p.name = "f{i}:" + str(s)',
        "    s + len(p.xs) % 7" if i == 0 else f"    s + f{i - 1}(a - 1, &p) % 7",
        "",
    ]
out += [
    'var p = P { x: 1, y: 0.0, name: "", xs: [] }',
    f"print(f{n - 1}(20, &p), len(p.xs), p.name)",
]
print("\n".join(out))
