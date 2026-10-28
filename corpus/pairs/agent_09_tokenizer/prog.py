# Tokenize statements: identifiers, numbers, operators.
import re

S = 19
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

ops = ["+", "-", "*", "/"]
parts = []
for _ in range(20000):
    parts.append(f"v{rnd() % 50} = ")
    for j in range(5):
        if j > 0:
            sp = "" if rnd() % 2 == 0 else " "
            parts.append(sp + ops[rnd() % 4] + sp)
        parts.append(f"x{rnd() % 100}" if rnd() % 2 == 0 else str(rnd() % 1000))
    parts.append(";\n")
idents = nums = opc = numsum = v7 = 0
for m in re.finditer(r"([a-z][a-z0-9]*)|(\d+)|(\S)", "".join(parts)):
    if m[1]:
        idents += 1
        v7 += m[1] == "v7"
    elif m[2]:
        nums += 1
        numsum += int(m[2])
    else:
        opc += 1
print(idents, nums, opc, numsum, v7)
