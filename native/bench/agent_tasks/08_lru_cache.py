# LRU cache simulation (most recent last).
from collections import OrderedDict

S = 17
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

cap = 50
cache = OrderedDict()
hits = 0
for _ in range(200000):
    k = rnd() % 120
    if k in cache:
        hits += 1
        cache.move_to_end(k)
    else:
        if len(cache) == cap:
            cache.popitem(last=False)
        cache[k] = True
keys = list(cache)
print(hits, 200000 - hits, keys[0], keys[-1])
