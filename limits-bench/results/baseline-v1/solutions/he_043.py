import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def fmt(x): return " ".join(map(str,x))
import itertools;x=ints(L[0]);print("true" if any(a+b==0 for a,b in itertools.combinations(x,2)) else "false")
