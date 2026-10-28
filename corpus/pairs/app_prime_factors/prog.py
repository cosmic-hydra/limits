# Factorise each integer on stdin; print "n = p^k * ..." and the divisor count.
import sys

def factor(n):
    fs = []
    d = 2
    while d * d <= n:
        k = 0
        while n % d == 0:
            n //= d
            k += 1
        if k:
            fs.append((d, k))
        d += 1
    if n > 1:
        fs.append((n, 1))
    return fs

for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    n = int(line)
    if n < 2:
        print(n, "has no prime factors")
        continue
    fs = factor(n)
    divisors = 1
    for _, k in fs:
        divisors *= k + 1
    print(n, "=", " * ".join(str(p) if k == 1 else f"{p}^{k}" for p, k in fs), "|", divisors, "divisors")
