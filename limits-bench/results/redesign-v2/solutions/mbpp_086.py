import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(w) for w in s.split()]
def b(v): print("true" if v else "false")
n=int(L[0]); print(3*n*(n-1)+1)
