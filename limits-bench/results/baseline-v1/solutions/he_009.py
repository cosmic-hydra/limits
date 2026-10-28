import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def strs(i):
    n=int(L[i]); return L[i+1:i+1+n], i+1+n
def ps(xs):
    print(len(xs))
    for x in xs: print(x)

o=[];m=None
for x in ints(L[0]):
    m=x if m is None else max(m,x);o.append(m)
print(" ".join(map(str,o)))
