import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def strs(i):
    n=int(L[i]); return L[i+1:i+1+n], i+1+n
def ps(xs):
    print(len(xs))
    for x in xs: print(x)

s=L[0];t=L[1]
print(sum(1 for i in range(len(s)-len(t)+1) if s[i:i+len(t)]==t))
