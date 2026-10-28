# Binary search: lower-bound lookups into a sorted list.
from bisect import bisect_left

S = 9
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

xs = []
v = 0
for _ in range(200000):
    v += 1 + rnd() % 5
    xs.append(v)
found = possum = 0
for _ in range(200000):
    q = rnd() % (v + 1)
    p = bisect_left(xs, q)
    possum += p
    if p < len(xs) and xs[p] == q:
        found += 1
print(found, possum)
