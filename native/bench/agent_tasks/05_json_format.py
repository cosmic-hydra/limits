# Format records as JSON-like lines.
import json
from dataclasses import dataclass, field, asdict

S = 5
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

@dataclass
class Item:
    id: int
    name: str
    score: int
    tags: list = field(default_factory=list)

colors = ["red", "green", "blue", "gold"]
lines = []
for i in range(60000):
    it = Item(i, f"n{rnd() % 1000}", rnd() % 100)
    it.tags = [colors[rnd() % 4] for _ in range(rnd() % 4)]
    lines.append(json.dumps(asdict(it)))
out = "\n".join(lines)
print(lines[0])
print(lines[1])
print(len(out))
