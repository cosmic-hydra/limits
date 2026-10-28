# Moving average over a window of 100.
S = 47
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

xs = [rnd() % 1000 for _ in range(500000)]
w = 100
s = sum(xs[:w])
avgs = [s // w]
for i in range(w, len(xs)):
    s += xs[i] - xs[i - w]
    avgs.append(s // w)
print(max(avgs), sum(a > 540 for a in avgs))
