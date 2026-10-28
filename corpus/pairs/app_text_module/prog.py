# Render "name qty price_cents" lines from stdin as an aligned table with a total.
import sys
from text import fmt

rows = []
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    name, qty, cents = line.split(" ")
    rows.append((name, int(qty), int(cents)))

def money(c):
    return str(c // 100) + "." + str(c % 100).rjust(2, "0")

w = max([len(r[0]) for r in rows] + [4])
print(fmt.pad_right("item", w), fmt.pad_left("qty", 4), fmt.pad_left("total", 9))
print(fmt.rule(w + 15))
grand = 0
for name, qty, cents in rows:
    grand += qty * cents
    print(fmt.pad_right(name, w), fmt.pad_left(str(qty), 4), fmt.pad_left(money(qty * cents), 9))
print(fmt.rule(w + 15))
print(fmt.pad_right("sum", w), fmt.pad_left("", 4), fmt.pad_left(money(grand), 9))
