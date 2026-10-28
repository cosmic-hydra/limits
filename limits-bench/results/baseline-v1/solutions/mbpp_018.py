import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def fmt(x): return " ".join(map(str,x))
s,t=L[0],L[1];print("".join(c for c in s if c not in t))
