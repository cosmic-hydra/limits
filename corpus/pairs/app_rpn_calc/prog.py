# Evaluate reverse-Polish integer expressions, one per stdin line.
import sys

def evaluate(expr):
    st = []
    for tok in expr.split(" "):
        if tok in ("+", "-", "*", "/"):
            if len(st) < 2:
                return None, "not enough operands for " + tok
            b, a = st.pop(), st.pop()
            if tok == "/" and b == 0:
                return None, "divide by zero"
            st.append(a + b if tok == "+" else a - b if tok == "-" else a * b if tok == "*" else int(a / b))
        else:
            try:
                st.append(int(tok))
            except ValueError:
                return None, "bad token " + tok
    if len(st) != 1:
        return None, str(len(st)) + " values left"
    return st[0], None

for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    v, err = evaluate(line)
    print(line, "=", v if err is None else "error: " + err)
