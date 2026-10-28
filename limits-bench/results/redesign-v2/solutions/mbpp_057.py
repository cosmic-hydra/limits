import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(w) for w in s.split()]
def b(v): print("true" if v else "false")
x=sorted(ints(L[0]),reverse=True); r=0
for d in x: r=r*10+d
print(r)
