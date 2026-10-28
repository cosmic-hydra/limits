def mean100(xs):
    return sum(xs) * 100 // len(xs)

def median2(xs):
    s = sorted(xs)
    n = len(s)
    return 2 * s[n // 2] if n % 2 == 1 else s[n // 2 - 1] + s[n // 2]

def mode(xs):
    best, best_n = xs[0], 0
    for x in sorted(xs):
        c = xs.count(x)
        if c > best_n:
            best, best_n = x, c
    return best
