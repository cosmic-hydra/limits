import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def strs(i):
    n=int(L[i]); return L[i+1:i+1+n], i+1+n
def ps(xs):
    print(len(xs))
    for x in xs: print(x)

xs=ints(L[0]);d=int(L[1]);o=[]
for i,x in enumerate(xs):
    if i: o.append(d)
    o.append(x)
print(" ".join(map(str,o)))
