# Histogram with text bars.
S = 41
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

buckets = [0] * 20
for _ in range(500000):
    a = rnd() % 1000
    buckets[(a + rnd() % 1000) // 2 // 50] += 1
most = max(buckets)
for i, b in enumerate(buckets):
    print(f"{i * 50}-{i * 50 + 49}", b, "#" * (b * 60 // most))
