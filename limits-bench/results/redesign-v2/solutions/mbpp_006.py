import sys
L=sys.stdin.read().split("\n")
def li(s): return [int(x) for x in s.split()]
def sh(x): print(" ".join(map(str,x)))
def b(x): print("true" if x else "false")
x=int(L[0])^int(L[1]);b(x!=0 and x&(x-1)==0)
