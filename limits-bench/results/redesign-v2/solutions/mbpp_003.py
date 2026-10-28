import sys
L=sys.stdin.read().split("\n")
def li(s): return [int(x) for x in s.split()]
def sh(x): print(" ".join(map(str,x)))
def b(x): print("true" if x else "false")
n=int(L[0]);b(any(n%d==0 for d in range(2,int(n**0.5)+1)) if n>=2 else False)
