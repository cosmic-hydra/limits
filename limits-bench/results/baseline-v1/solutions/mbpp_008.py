import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def fmt(x): return " ".join(map(str,x))
print(fmt([x*x for x in ints(L[0])]))
