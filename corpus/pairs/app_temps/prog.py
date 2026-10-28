# Convert temperature readings like "21.5C" or "70F" from stdin; summarise in Celsius.
import sys

def parse(s):
    if len(s) < 2 or s[-1] not in "CF":
        return None
    try:
        v = float(s[:-1])
    except ValueError:
        return None
    return v if s[-1] == "C" else (v - 32.0) * 5.0 / 9.0

vals = []
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    c = parse(line)
    if c is None:
        print("bad reading:", line)
    else:
        vals.append(c)
        print(line, "=", c, "C")
if vals:
    total = 0.0
    for v in vals:  # plain left-to-right sum (sum() on floats is compensated since 3.12)
        total += v
    print("min", min(vals), "max", max(vals), "mean", total / len(vals))
else:
    print("no readings")
