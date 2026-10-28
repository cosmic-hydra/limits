# Order build steps: stdin lines "task: dep dep ...". Prints a deterministic topological order or the cycle members.
import sys

tasks, deps = [], []
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    name, rest = line.split(":")
    tasks.append(name)
    deps.append([d for d in rest.split(" ") if d != ""])
for ds in deps:
    for d in ds:
        if d not in tasks:
            tasks.append(d)
            deps.append([])
done = []
while len(done) < len(tasks):
    ready = sorted(t for i, t in enumerate(tasks) if t not in done and all(d in done for d in deps[i]))
    if not ready:
        print("cycle among:", " ".join(sorted(t for t in tasks if t not in done)))
        break
    done.append(ready[0])
print("order:", " ".join(done))
