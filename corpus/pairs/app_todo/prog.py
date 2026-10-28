# Tiny todo manager: state in todo.txt, command from args (add TEXT / done N / list).
import sys, os

PATH = "todo.txt"
items = []
if os.path.exists(PATH):
    for line in open(PATH).read().split("\n"):
        if line != "":
            items.append([line[0] == "x", line[2:]])

args = sys.argv[1:]
cmd = args[0] if args else "list"
if cmd == "add":
    items.append([False, " ".join(args[1:])])
    print("added", len(items))
elif cmd == "done":
    n = int(args[1])
    if 1 <= n <= len(items):
        items[n - 1][0] = True
        print("done", n)
    else:
        print("no item", n)
for i, (done, text) in enumerate(items, 1):
    print(f"{i} [{'x' if done else ' '}] {text}")
with open(PATH, "w") as f:
    f.write("".join(("x " if d else "- ") + t + "\n" for d, t in items))
print(open(PATH).read().count("\n"), "saved")
