# Shortest paths from node 0. stdin: "N M", then M lines "u v w" (undirected).
import sys

lines = sys.stdin.read().split("\n")
n, m = map(int, lines[0].split(" "))
adj = [[] for _ in range(n)]
for i in range(1, m + 1):
    u, v, w = map(int, lines[i].split(" "))
    adj[u].append((v, w))
    adj[v].append((u, w))
INF = None
dist = [INF] * n
prev = [-1] * n
done = [False] * n
dist[0] = 0
for _ in range(n):
    best = -1
    for i in range(n):
        if not done[i] and dist[i] is not None and (best < 0 or dist[i] < dist[best]):
            best = i
    if best < 0:
        break
    done[best] = True
    for v, w in adj[best]:
        if dist[v] is None or dist[best] + w < dist[v]:
            dist[v] = dist[best] + w
            prev[v] = best
for i in range(n):
    if dist[i] is None:
        print(i, "unreachable")
        continue
    path = [i]
    while prev[path[-1]] >= 0:
        path.append(prev[path[-1]])
    print(i, dist[i], "via", "-".join(str(x) for x in reversed(path)))
