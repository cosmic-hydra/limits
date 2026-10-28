# Caesar-shift stdin text by the amount in argv[1] (negative decodes); keeps case and non-letters.
import sys

shift = int(sys.argv[1]) if len(sys.argv) > 1 else 13
lower = "abcdefghijklmnopqrstuvwxyz"
upper = lower.upper()
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    out = ""
    for c in line:
        if c in lower:
            out += lower[(lower.index(c) + shift) % 26]
        elif c in upper:
            out += upper[(upper.index(c) + shift) % 26]
        else:
            out += c
    print(out)
