# Decode run-length strings like "3a2b1c" from stdin; report malformed input.
import sys

def decode(s):
    out, num = [], ""
    for c in s:
        if c.isdigit():
            num += c
        else:
            if num == "":
                raise ValueError("missing count before '" + c + "'")
            out.append(c * int(num))
            num = ""
    if num != "":
        raise ValueError("dangling count " + num)
    return "".join(out)

for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    try:
        d = decode(line)
        print(line, "->", d, len(d))
    except ValueError as e:
        print(line, "-> error:", e)
