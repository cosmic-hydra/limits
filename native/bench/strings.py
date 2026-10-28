total = 0
for r in range(20):
    s = ""
    for i in range(2000):
        s = s + str(i) + ","
    total += len(s)
parts = []
for i in range(1000000):
    parts.append("item" + str(i))
n = 0
for p in parts:
    n += len(p)
print(total, n)
