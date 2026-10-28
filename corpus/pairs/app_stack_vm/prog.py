# A tiny stack machine: one instruction per stdin line (push N, add, sub, mul, div, dup, swap, print).
import sys

def run(prog):
    stack = []
    out = []
    for pc, ins in enumerate(prog):
        op = ins.split(" ")
        need = {"add": 2, "sub": 2, "mul": 2, "div": 2, "dup": 1, "swap": 2, "print": 1}.get(op[0], 0)
        if op[0] != "push" and op[0] not in ("add", "sub", "mul", "div", "dup", "swap", "print"):
            return out, f"pc {pc}: unknown op {op[0]}"
        if len(stack) < need:
            return out, f"pc {pc}: stack underflow"
        if op[0] == "push":
            stack.append(int(op[1]))
        elif op[0] == "dup":
            stack.append(stack[-1])
        elif op[0] == "swap":
            stack[-1], stack[-2] = stack[-2], stack[-1]
        elif op[0] == "print":
            out.append(str(stack[-1]))
        else:
            b = stack.pop()
            a = stack.pop()
            if op[0] == "div" and b == 0:
                return out, f"pc {pc}: division by zero"
            if op[0] == "add": stack.append(a + b)
            elif op[0] == "sub": stack.append(a - b)
            elif op[0] == "mul": stack.append(a * b)
            else: stack.append(int(a / b))
    return out, None

prog = [l for l in sys.stdin.read().split("\n") if l != ""]
out, err = run(prog)
for o in out:
    print(o)
print("error:", err) if err else print("ok")
