import sys
L=sys.stdin.read().split("\n")
def ints(i): return [int(t) for t in L[i].split()] if L[i] else []
def b(v): print("true" if v else "false")
x=int(L[0])
def f(x):
    if x%2==0: return False
    if x==1: return True
    x+=1; p=0
    while x%2==0 and x!=0:
        x//=2; p+=1
        if p==x: return True
    return False
b(f(x))
