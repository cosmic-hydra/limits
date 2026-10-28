# Merge the sorted number files named in argv into one sorted stream; print it and the duplicates.
import sys

lists = [[int(x) for x in open(p).read().split("\n") if x != ""] for p in sys.argv[1:]]
pos = [0] * len(lists)
merged = []
while True:
    best = -1
    for i, xs in enumerate(lists):
        if pos[i] < len(xs) and (best < 0 or xs[pos[i]] < lists[best][pos[best]]):
            best = i
    if best < 0:
        break
    merged.append(lists[best][pos[best]])
    pos[best] += 1
print(" ".join(map(str, merged)))
dups = sorted(set(x for i, x in enumerate(merged) if i > 0 and merged[i - 1] == x))
print("duplicates:", " ".join(map(str, dups)) if dups else "none")
