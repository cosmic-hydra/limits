# Sum distances like "5km", "300m", "2mi" from stdin and print the total in metres.
import sys

FACTOR = {"m": 1, "km": 1000, "mi": 1609, "ft": 0}

def parse(tok):
    i = 0
    while i < len(tok) and tok[i].isdigit():
        i += 1
    num, unit = tok[:i], tok[i:]
    if num == "":
        return None, "no number in " + tok
    if unit == "ft":
        return int(num) * 3048 // 10000, None
    if unit not in FACTOR:
        return None, "unknown unit '" + unit + "'"
    return int(num) * FACTOR[unit], None

total = 0
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    for tok in line.split(" "):
        m, err = parse(tok)
        if err:
            print("skip:", err)
        else:
            total += m
            print(tok, "=", m, "m")
print("total", total, "m")
