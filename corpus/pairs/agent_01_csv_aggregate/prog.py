# Parse CSV-ish text into structs and aggregate per department.
from dataclasses import dataclass

S = 42
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

@dataclass
class Rec:
    name: str
    dept: str
    amount: int

depts = ["eng", "ops", "sales", "hr", "legal", "it", "fin", "mkt"]
text = "\n".join(f"u{i},{depts[rnd() % 8]},{rnd() % 1000}" for i in range(200000))
recs = []
for line in text.split("\n"):
    name, dept, amount = line.split(",")
    recs.append(Rec(name, dept, int(amount)))
totals = dict.fromkeys(depts, 0)
counts = dict.fromkeys(depts, 0)
for r in recs:
    totals[r.dept] += r.amount
    counts[r.dept] += 1
for d in depts:
    print(d, counts[d], totals[d], totals[d] // counts[d])
