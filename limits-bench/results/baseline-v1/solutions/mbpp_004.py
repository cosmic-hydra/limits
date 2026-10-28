import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def fmt(x): return " ".join(map(str,x))
print(fmt(sorted(ints(L[0]),reverse=True)[:int(L[1])]))
