# Join users.csv (id,name) with orders.csv (id,user_id,cents); print spend per user.
import sys

users = [l.split(",") for l in open(sys.argv[1]).read().split("\n")[1:] if l != ""]
orders = [l.split(",") for l in open(sys.argv[2]).read().split("\n")[1:] if l != ""]
orphans = 0
spend = {u[0]: 0 for u in users}
count = {u[0]: 0 for u in users}
for o in orders:
    if o[1] not in spend:
        orphans += 1
        continue
    spend[o[1]] += int(o[2])
    count[o[1]] += 1
ranked = sorted(users, key=lambda u: (-spend[u[0]], u[1]))
for uid, name in ranked:
    print(f"{name:<8}{count[uid]:>3}{spend[uid]:>8}")
print("orphan orders:", orphans)
