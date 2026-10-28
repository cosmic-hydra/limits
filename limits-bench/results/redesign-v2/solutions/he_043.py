import sys
L=sys.stdin.read().split("\n")
def li(s): return [int(x) for x in s.split()]
def sh(x): print(" ".join(map(str,x)))
def b(x): print("true" if x else "false")
l=li(L[0]);import itertools;b(any(sum(t)==0 for t in itertools.combinations(l,2)))
