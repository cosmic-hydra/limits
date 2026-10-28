# Validate "username,email,age" rows from stdin; list every problem per row.
import sys

def problems(row):
    out = []
    parts = row.split(",")
    if len(parts) != 3:
        return ["expected 3 fields, got " + str(len(parts))]
    user, email, age = parts
    if not (3 <= len(user) <= 12):
        out.append("username length " + str(len(user)))
    if email.count("@") != 1 or "." not in email.split("@")[1]:
        out.append("bad email " + email)
    try:
        a = int(age)
        if a < 13 or a > 130:
            out.append("age out of range " + str(a))
    except ValueError:
        out.append("age not a number")
    return out

ok = 0
for i, row in enumerate(sys.stdin.read().split("\n"), 1):
    if row == "":
        break
    ps = problems(row)
    if not ps:
        ok += 1
    else:
        print("row " + str(i) + ": " + "; ".join(ps))
print(ok, "valid")
