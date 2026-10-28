# Integer matrix multiply.
S = 11
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

n = 150
a = [[rnd() % 10 for _ in range(n)] for _ in range(n)]
b = [[rnd() % 10 for _ in range(n)] for _ in range(n)]
bt = list(zip(*b))
c = [[sum(x * y for x, y in zip(row, col)) for col in bt] for row in a]
print(sum(c[i][i] for i in range(n)), sum(map(sum, c)))
