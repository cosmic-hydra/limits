import sys
L=sys.stdin.read().split("\n")
def ints(i): return [int(t) for t in L[i].split()] if L[i] else []
def b(v): print("true" if v else "false")
x=ints(0); b(all(x[i]<=x[i+1] for i in range(len(x)-1)) or all(x[i]>=x[i+1] for i in range(len(x)-1)))
