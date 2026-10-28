import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def strs(i):
    n=int(L[i]); return L[i+1:i+1+n], i+1+n
def ps(xs):
    print(len(xs))
    for x in xs: print(x)

out=[];cur="";d=0
for c in L[0]:
    if c=="(": d+=1;cur+=c
    elif c==")":
        d-=1;cur+=c
        if d==0: out.append(cur);cur=""
ps(out)
