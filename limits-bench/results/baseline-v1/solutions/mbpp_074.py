import sys
L=sys.stdin.read().split("\n")
def ints(i): return [int(t) for t in L[i].split()] if L[i] else []
def b(v): print("true" if v else "false")
i=0
def rd():
    global i
    n=int(L[i]); v=L[i+1:i+1+n]; i+=1+n; return v
c=rd(); p=rd()
def f():
    if len(c)!=len(p): return False
    if len(set(c))!=len(set(p)): return False
    d={}
    for a,q in zip(c,p):
        if d.setdefault(q,a)!=a: return False
    return True
b(f())
