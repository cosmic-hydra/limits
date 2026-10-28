# BFS shortest path on a grid with walls.
from collections import deque

S = 13
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

n = 400
wall = [rnd() % 100 < 25 for _ in range(n * n)]
wall[0] = wall[n * n - 1] = False
dist = [-1] * (n * n)
dist[0] = 0
q = deque([0])
reached = 1
while q:
    cur = q.popleft()
    r, c = divmod(cur, n)
    for nr, nc in ((r + 1, c), (r - 1, c), (r, c + 1), (r, c - 1)):
        if 0 <= nr < n and 0 <= nc < n:
            nx = nr * n + nc
            if not wall[nx] and dist[nx] < 0:
                dist[nx] = dist[cur] + 1
                q.append(nx)
                reached += 1
print(reached, dist[n * n - 1])
