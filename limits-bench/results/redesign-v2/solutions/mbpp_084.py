import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(w) for w in s.split()]
def b(v): print("true" if v else "false")
n=int(L[0]); p=[0,1,1]
for i in range(3,n+1): p.append(p[p[i-1]]+p[i-p[i-1]])
print(p[n])
