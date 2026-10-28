# Evaluate arithmetic expressions (mod 1000003) with two stacks.
S = 53
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

M = 1000003
PREC = {"*": 2, "+": 1, "-": 1, "(": 0}

def gen(toks, depth):
    for i in range(1 + rnd() % 3):
        if i > 0:
            toks.append("+-*"[rnd() % 3])
        if depth < 3 and rnd() % 3 == 0:
            toks.append("(")
            gen(toks, depth + 1)
            toks.append(")")
        else:
            toks.append(str(1 + rnd() % 9))

def apply(nums, ops):
    op, b, a = ops.pop(), nums.pop(), nums.pop()
    nums.append((a + b) % M if op == "+" else (a - b) % M if op == "-" else a * b % M)

def evaluate(text):
    nums, ops = [], []
    for t in text.split(" "):
        if t == "(":
            ops.append(t)
        elif t == ")":
            while ops[-1] != "(":
                apply(nums, ops)
            ops.pop()
        elif t in PREC:
            while ops and PREC[ops[-1]] >= PREC[t]:
                apply(nums, ops)
            ops.append(t)
        else:
            nums.append(int(t))
    while ops:
        apply(nums, ops)
    return nums[0]

total = 0
for _ in range(40000):
    toks = []
    gen(toks, 0)
    total += evaluate(" ".join(toks))
print(total)
