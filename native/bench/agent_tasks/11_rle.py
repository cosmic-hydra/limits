# Run-length encode a string.
from itertools import groupby

S = 29
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

alpha = "abcd"
chars = []
while len(chars) < 300000:
    ch = alpha[rnd() % 4]
    chars.extend(ch * (1 + rnd() % 6))
s = "".join(chars)
pieces = [f"{len(list(g))}{c}" for c, g in groupby(s)]
print(len("".join(pieces)), len(pieces), "".join(pieces[:8]))
