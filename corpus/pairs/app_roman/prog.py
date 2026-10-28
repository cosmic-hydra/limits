# Convert each stdin line between Roman numerals and integers (1..3999).
import sys

VALS = [1000, 900, 500, 400, 100, 90, 50, 40, 10, 9, 5, 4, 1]
SYMS = ["M", "CM", "D", "CD", "C", "XC", "L", "XL", "X", "IX", "V", "IV", "I"]

def to_roman(n):
    if n < 1 or n > 3999:
        return None, "out of range: " + str(n)
    out = ""
    for v, s in zip(VALS, SYMS):
        while n >= v:
            out += s
            n -= v
    return out, None

def from_roman(s):
    single = {"I": 1, "V": 5, "X": 10, "L": 50, "C": 100, "D": 500, "M": 1000}
    total = 0
    for i, c in enumerate(s):
        if c not in single:
            return None, "bad digit: " + c
        v = single[c]
        if i + 1 < len(s) and s[i + 1] in single and single[s[i + 1]] > v:
            total -= v
        else:
            total += v
    back, _ = to_roman(total)
    if back != s:
        return None, "not canonical: " + s
    return total, None

for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    if line.lstrip("-").isdigit():
        v, err = to_roman(int(line))
    else:
        v, err = from_roman(line)
    print(line, "->", v if err is None else "error: " + err)
