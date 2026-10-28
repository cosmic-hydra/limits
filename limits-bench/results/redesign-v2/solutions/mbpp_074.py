import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(w) for w in s.split()]
def b(v): print("true" if v else "false")
n=int(L[0]); c=L[1:1+n]; m=int(L[1+n]); p=L[2+n:2+n+m]
b(len(c)==len(p) and all((c[i]==c[j])==(p[i]==p[j]) for i in range(n) for j in range(n)))
