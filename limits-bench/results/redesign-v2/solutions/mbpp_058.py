import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(w) for w in s.split()]
def b(v): print("true" if v else "false")
x=int(L[0]);y=int(L[1]); b((x<0)!=(y<0))
