# Bank ledger simulation with overdraft rejection.
from dataclasses import dataclass

S = 43
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

@dataclass
class Account:
    id: int
    balance: int

accts = [Account(i, 1000) for i in range(10)]
rejected = 0
for _ in range(300000):
    kind = rnd() % 3
    a = accts[rnd() % 10]
    b = accts[rnd() % 10]
    amt = rnd() % 500
    if kind == 0:
        a.balance += amt
    elif a.balance < amt or (kind == 2 and a is b):
        rejected += 1
    else:
        a.balance -= amt
        if kind == 2:
            b.balance += amt
print([a.balance for a in accts], rejected)
