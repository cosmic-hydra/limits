# Bucket the integers on stdin (0..99) into tens and draw a bar chart scaled to width 20.
import sys

xs = []
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    xs += [int(w) for w in line.split(" ") if w != ""]
buckets = [0] * 10
bad = 0
for x in xs:
    if 0 <= x <= 99:
        buckets[x // 10] += 1
    else:
        bad += 1
top = max(buckets)
for i, c in enumerate(buckets):
    bar = "#" * (c * 20 // top) if top else ""
    print(f"{i * 10:2d}-{i * 10 + 9:2d} | {bar} {c}" if c else f"{i * 10:2d}-{i * 10 + 9:2d} |")
print("out of range:", bad)
