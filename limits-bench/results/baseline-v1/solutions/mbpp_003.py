import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def fmt(x): return " ".join(map(str,x))
n=int(L[0]);print("true" if n<2 or any(n%d==0 for d in range(2,int(n**0.5)+1)) else "false")
