# Read "name,score,score,..." rows from stdin; rank students by average (x100, truncated).
import sys

rows = []
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    parts = line.split(",")
    scores = [int(p) for p in parts[1:]]
    avg100 = sum(scores) * 100 // len(scores) if scores else 0
    rows.append((parts[0], avg100, max(scores) if scores else 0))
rows.sort(key=lambda r: (-r[1], r[0]))
for rank, (name, avg, best) in enumerate(rows, 1):
    grade = "A" if avg >= 9000 else "B" if avg >= 8000 else "C" if avg >= 7000 else "F"
    print(f"{rank}. {name} {avg // 100}.{avg % 100:02d} best={best} {grade}")
print("passed:", len([r for r in rows if r[1] >= 7000]), "of", len(rows))
