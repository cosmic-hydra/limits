import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def strs(i):
    n=int(L[i]); return L[i+1:i+1+n], i+1+n
def ps(xs):
    print(len(xs))
    for x in xs: print(x)

o=[]
for g in L[0].split(" "):
    if g:
        d=m=0
        for c in g:
            if c=="(": d+=1;m=max(m,d)
            elif c==")": d-=1
        o.append(m)
print(" ".join(map(str,o)))
