import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(w) for w in s.split()]
def b(v): print("true" if v else "false")
x=int(L[0]); t=x+1; n=1; r=False
while x>=1 and n*2**n<=t:
    r=r or n*2**n==t; n+=1
b(r)
