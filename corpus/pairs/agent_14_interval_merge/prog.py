# Merge overlapping intervals.
S = 37
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

iv = []
for _ in range(200000):
    s = rnd() * 300
    iv.append((s, s + rnd() % 1000))
merged = []
for s, e in sorted(iv):
    if merged and s <= merged[-1][1]:
        merged[-1][1] = max(merged[-1][1], e)
    else:
        merged.append([s, e])
lens = [e - s for s, e in merged]
print(len(merged), sum(lens), max(lens))
