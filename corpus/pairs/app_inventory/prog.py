# Apply inventory commands from stdin: add NAME QTY, remove NAME QTY, show NAME.
import sys

stock = []  # list of [name, qty]

def find(name):
    for i, (n, _) in enumerate(stock):
        if n == name:
            return i
    return None

def apply(line):
    parts = line.split(" ")
    if parts[0] == "add" and len(parts) == 3:
        i = find(parts[1])
        if i is None:
            stock.append([parts[1], int(parts[2])])
        else:
            stock[i][1] += int(parts[2])
        return "ok"
    if parts[0] == "remove" and len(parts) == 3:
        i = find(parts[1])
        if i is None:
            return "error: no " + parts[1]
        if stock[i][1] < int(parts[2]):
            return "error: only " + str(stock[i][1]) + " " + parts[1]
        stock[i][1] -= int(parts[2])
        return "ok"
    if parts[0] == "show" and len(parts) == 2:
        i = find(parts[1])
        return parts[1] + " " + str(0 if i is None else stock[i][1])
    return "error: bad command: " + line

for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    print(apply(line))
print("total", sum(q for _, q in stock))
