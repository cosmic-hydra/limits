# Sum of Levenshtein distances over random word pairs.
S = 31
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

def word():
    return "".join("abcdef"[rnd() % 6] for _ in range(10 + rnd() % 21))

def lev(a, b):
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j - 1] + (ca != cb), prev[j] + 1, cur[j - 1] + 1))
        prev = cur
    return prev[-1]

total = 0
for _ in range(1500):
    s = word()
    t = word()
    total += lev(s, t)
print(total)
