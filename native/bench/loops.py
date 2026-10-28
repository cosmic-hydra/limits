s = 0
for i in range(10000):
    for j in range(10000):
        s += (i * j + j) % 7
print(s)
