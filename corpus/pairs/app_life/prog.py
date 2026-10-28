# Run Conway's Game of Life on the grid from stdin ("N" generations on the first line, then rows of . and #).
import sys

lines = [l for l in sys.stdin.read().split("\n") if l != ""]
gens = int(lines[0])
g = [list(r) for r in lines[1:]]
h, w = len(g), len(g[0])
for _ in range(gens):
    n = [["."] * w for _ in range(h)]
    for y in range(h):
        for x in range(w):
            c = sum(1 for dy in (-1, 0, 1) for dx in (-1, 0, 1)
                    if (dy or dx) and 0 <= y + dy < h and 0 <= x + dx < w and g[y + dy][x + dx] == "#")
            if c == 3 or (c == 2 and g[y][x] == "#"):
                n[y][x] = "#"
    g = n
for r in g:
    print("".join(r))
print("alive", sum(r.count("#") for r in g))
