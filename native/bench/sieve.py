n = 10000000
isp = [True] * (n + 1)
count = 0
for i in range(2, n + 1):
    if isp[i]:
        count += 1
        j = i * i
        while j <= n:
            isp[j] = False
            j += i
print(count)
