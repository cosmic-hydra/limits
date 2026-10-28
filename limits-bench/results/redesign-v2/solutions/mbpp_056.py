import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(w) for w in s.split()]
def b(v): print("true" if v else "false")
n=int(L[0]); b(n>=0 and 2*int(str(n)[::-1])==n+1)
