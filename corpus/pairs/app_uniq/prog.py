# uniq-like filter over stdin lines. Flags: -c (prefix counts), -d (only repeated), -i (ignore case).
import sys

flags = sys.argv[1:]
count = "-c" in flags
only_dup = "-d" in flags
icase = "-i" in flags
groups = []  # [key, first_line, n]
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    key = line.lower() if icase else line
    if groups and groups[-1][0] == key:
        groups[-1][2] += 1
    else:
        groups.append([key, line, 1])
for key, first, n in groups:
    if only_dup and n < 2:
        continue
    print(f"{n:>4} {first}" if count else first)
