import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def fmt(x): return " ".join(map(str,x))
x=int(L[0])^int(L[1]);print("true" if x and x&(x-1)==0 else "false")
