# Check bracket balance of each stdin line; report the first error column (1-based).
import sys

PAIRS = {")": "(", "]": "[", "}": "{"}

def check(s):
    stack = []
    for i, c in enumerate(s):
        if c in "([{":
            stack.append((c, i))
        elif c in PAIRS:
            if not stack or stack[-1][0] != PAIRS[c]:
                return i + 1
            stack.pop()
    return stack[-1][1] + 1 if stack else None

for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    col = check(line)
    print("ok" if col is None else "error at " + str(col), line)
