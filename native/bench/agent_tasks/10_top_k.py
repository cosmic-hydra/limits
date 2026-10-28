# Top 10 largest values, descending.
import heapq

S = 23
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

xs = [rnd() * 32768 + rnd() for _ in range(300000)]
print(heapq.nlargest(10, xs))
