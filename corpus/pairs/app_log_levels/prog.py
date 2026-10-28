# Count log lines per level in the file named by the first argument.
import sys

LEVELS = ["DEBUG", "INFO", "WARN", "ERROR"]

def level_of(line):
    parts = line.split(" ")
    if len(parts) >= 2 and parts[1] in LEVELS:
        return parts[1]
    return None

counts = [0, 0, 0, 0]
unknown = 0
errors = []
for line in open(sys.argv[1]).read().split("\n"):
    if line == "":
        continue
    lv = level_of(line)
    if lv is None:
        unknown += 1
        continue
    counts[LEVELS.index(lv)] += 1
    if lv == "ERROR":
        errors.append(line.split(" ", 2)[2] if len(line.split(" ")) > 2 else "")
for lv, c in zip(LEVELS, counts):
    print(lv, c)
print("unparsed", unknown)
for e in errors:
    print("!", e)
