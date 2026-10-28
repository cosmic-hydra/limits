import sys
L=sys.stdin.read().split("\n")
def ints(i): return [int(t) for t in L[i].split()] if L[i] else []
def b(v): print("true" if v else "false")
n=int(L[0]); print(3*n*n-2*n)
