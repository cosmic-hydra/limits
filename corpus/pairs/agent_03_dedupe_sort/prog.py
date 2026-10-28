# Dedupe and sort a list of ints.
S = 3
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

xs = [rnd() % 100000 for _ in range(300000)]
u = sorted(set(xs))
print(len(u), u[0], u[1], u[2], u[-1], sum(u))
