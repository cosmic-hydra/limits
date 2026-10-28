# Accounts from stdin header "N", then N "name balance" lines, then "from to amount" transfers.
import sys

def transfer(bal, a, b, amt):
    assert amt > 0
    if bal[a] < amt:
        return False
    bal[a] -= amt
    bal[b] += amt
    return True

lines = sys.stdin.read().split("\n")
n = int(lines[0])
names = []
bal = []
for i in range(1, n + 1):
    name, b = lines[i].split(" ")
    names.append(name)
    bal.append(int(b))
before = sum(bal)
for line in lines[n + 1:]:
    if line == "":
        break
    f, t, amt = line.split(" ")
    if f not in names or t not in names:
        print("unknown account in:", line)
        continue
    ok = transfer(bal, names.index(f), names.index(t), int(amt))
    print(("ok" if ok else "insufficient") + ":", line)
for name, b in zip(names, bal):
    print(name, b)
print("conserved:", "true" if sum(bal) == before else "false")
