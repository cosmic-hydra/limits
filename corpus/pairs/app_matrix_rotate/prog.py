# Read an N x N matrix from stdin and rotate it 90 degrees clockwise K times, in place.
import sys

def rotate(m):
    n = len(m)
    for i in range(n):
        for j in range(i + 1, n):
            m[i][j], m[j][i] = m[j][i], m[i][j]
    for row in m:
        row.reverse()

lines = sys.stdin.read().split("\n")
n, k = int(lines[0].split(" ")[0]), int(lines[0].split(" ")[1])
m = [[int(x) for x in lines[1 + i].split(" ")] for i in range(n)]
for _ in range(k % 4):
    rotate(m)
for row in m:
    print(" ".join(str(x) for x in row))
print("trace", sum(m[i][i] for i in range(n)))
