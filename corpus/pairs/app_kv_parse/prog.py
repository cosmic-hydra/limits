# Parse "key=int;key=int" records from stdin; print the sum per record or the first error.
import sys

def parse_record(line):
    total = 0
    keys = []
    for field in line.split(";"):
        kv = field.split("=")
        if len(kv) != 2 or kv[0] == "":
            raise ValueError("bad field '" + field + "'")
        if kv[0] in keys:
            raise ValueError("duplicate key " + kv[0])
        try:
            v = int(kv[1])
        except ValueError:
            raise ValueError("not a number: " + kv[1])
        keys.append(kv[0])
        total += v
    return keys, total

for n, line in enumerate(sys.stdin.read().split("\n"), 1):
    if line == "":
        break
    try:
        keys, total = parse_record(line)
        print(n, ",".join(sorted(keys)), total)
    except ValueError as e:
        print(n, "error:", e)
