import sys
L=sys.stdin.read().split("\n")
def li(s): return [int(x) for x in s.split()]
def sh(x): print(" ".join(map(str,x)))
def b(x): print("true" if x else "false")
def p(x):
    return x>=2 and all(x%d for d in range(2,int(x**0.5)+1))
n=int(L[0]);a,c=0,1
while n>0:
    a,c=c,a+c
    if p(c): n-=1
print(c)
