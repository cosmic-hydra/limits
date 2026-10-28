import sys
L=sys.stdin.read().split("\n")
def ints(i): return [int(t) for t in L[i].split()] if L[i] else []
def b(v): print("true" if v else "false")
n=int(L[0]); m=n; r=0
while m>0: r=r*10+m%10; m//=10
b(2*r==n+1)
