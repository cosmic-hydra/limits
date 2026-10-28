# Group the words on stdin into anagram classes; print classes with 2+ members, largest first.
import sys

words = [w for w in sys.stdin.read().split() if w != ""]
groups = []  # [key, [words]]
for w in words:
    key = "".join(sorted(w.lower()))
    for g in groups:
        if g[0] == key:
            if w not in g[1]:
                g[1].append(w)
            break
    else:
        groups.append([key, [w]])
big = [g for g in groups if len(g[1]) > 1]
big.sort(key=lambda g: (-len(g[1]), g[0]))
for key, ws in big:
    print(key + ":", " ".join(sorted(ws)))
print(len(big), "groups,", len(groups), "keys")
