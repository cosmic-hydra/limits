import sys
L=sys.stdin.read().split("\n")
def ints(i): return [int(t) for t in L[i].split()] if L[i] else []
def b(v): print("true" if v else "false")
n=int(L[0]); p=[0,1,1]
for i in range(3,n+1): p.append(p[p[i-1]]+p[i-p[i-1]])
print(1 if n<=2 else p[n])
