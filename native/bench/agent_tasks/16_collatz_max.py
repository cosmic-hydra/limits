# Longest Collatz chain below a limit.
best = best_n = 0
for n in range(1, 300000):
    x, steps = n, 1
    while x != 1:
        x = x // 2 if x % 2 == 0 else 3 * x + 1
        steps += 1
    if steps > best:
        best, best_n = steps, n
print(best_n, best)
