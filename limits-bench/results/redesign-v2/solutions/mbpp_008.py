import sys
L=sys.stdin.read().split("\n")
def li(s): return [int(x) for x in s.split()]
def sh(x): print(" ".join(map(str,x)))
def b(x): print("true" if x else "false")
sh([x*x for x in li(L[0])])
