# Price payment events from stdin: "card CENTS", "cash CENTS", "refund CENTS", "voucher CODE".
import sys

def fee(kind, cents):
    if kind == "card":
        return cents * 3 // 100
    return 0

balance = 0
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    kind, arg = line.split(" ")
    if kind in ("card", "cash"):
        net = int(arg) - fee(kind, int(arg))
    elif kind == "refund":
        net = -int(arg)
    elif kind == "voucher":
        net = 500 if arg.startswith("V") else 0
    else:
        print("unknown:", line)
        continue
    balance += net
    print(kind, net)
print("balance", balance)
