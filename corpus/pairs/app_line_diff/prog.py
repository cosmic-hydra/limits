# Line diff of two files (argv): LCS-based, prints "  " / "- " / "+ " lines and a summary.
import sys

a = open(sys.argv[1]).read().split("\n")
b = open(sys.argv[2]).read().split("\n")
if a and a[-1] == "": a.pop()
if b and b[-1] == "": b.pop()
n, m = len(a), len(b)
L = [[0] * (m + 1) for _ in range(n + 1)]
for i in range(n - 1, -1, -1):
    for j in range(m - 1, -1, -1):
        L[i][j] = L[i + 1][j + 1] + 1 if a[i] == b[j] else max(L[i + 1][j], L[i][j + 1])
i = j = 0
added = removed = 0
while i < n or j < m:
    if i < n and j < m and a[i] == b[j]:
        print("  " + a[i]); i += 1; j += 1
    elif j < m and (i == n or L[i][j + 1] >= L[i + 1][j]):
        print("+ " + b[j]); j += 1; added += 1
    else:
        print("- " + a[i]); i += 1; removed += 1
print(f"{added} added, {removed} removed")
