# Insert stdin integers into a binary search tree (array-backed); print in-order, height, lookups.
import sys

keys, left, right = [], [], []

def insert(k):
    if not keys:
        keys.append(k); left.append(None); right.append(None)
        return
    i = 0
    while True:
        if k == keys[i]:
            return
        side = left if k < keys[i] else right
        if side[i] is None:
            side[i] = len(keys)
            keys.append(k); left.append(None); right.append(None)
            return
        i = side[i]

def inorder(i, out):
    if i is None:
        return
    inorder(left[i], out)
    out.append(keys[i])
    inorder(right[i], out)

def height(i):
    return 0 if i is None else 1 + max(height(left[i]), height(right[i]))

lines = sys.stdin.read().split("\n")
for w in lines[0].split(" "):
    if w != "":
        insert(int(w))
out = []
inorder(0 if keys else None, out)
print("sorted:", " ".join(map(str, out)))
print("height:", height(0 if keys else None))
for w in lines[1].split(" "):
    if w != "":
        print(w, "found" if int(w) in keys else "missing")
