# Read sales.csv (region,item,units,cents), write report.txt with per-region totals, then echo it.
import sys

rows = [l.split(",") for l in open("sales.csv").read().split("\n")[1:] if l != ""]
regions = []
for r in rows:
    if r[0] not in regions:
        regions.append(r[0])
regions.sort()
out = []
grand = 0
for reg in regions:
    units = sum(int(r[2]) for r in rows if r[0] == reg)
    cents = sum(int(r[2]) * int(r[3]) for r in rows if r[0] == reg)
    grand += cents
    out.append(f"{reg}: {units} units, {cents // 100}.{cents % 100:02d}")
out.append(f"TOTAL: {grand // 100}.{grand % 100:02d}")
with open("report.txt", "w") as f:
    f.write("\n".join(out) + "\n")
print(open("report.txt").read(), end="")
print("wrote", len(out), "lines")
