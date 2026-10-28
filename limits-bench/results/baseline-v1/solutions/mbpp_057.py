import sys
L=sys.stdin.read().split("\n")
def ints(i): return [int(t) for t in L[i].split()] if L[i] else []
def b(v): print("true" if v else "false")
x=sorted(ints(0),reverse=True); r=0
for d in x: r=r*10+d
print(r)
