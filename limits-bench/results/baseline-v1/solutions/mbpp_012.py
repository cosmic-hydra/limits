import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def fmt(x): return " ".join(map(str,x))
n=int(L[0]);M=[ints(L[1+i]) for i in range(n)];M.sort(key=sum);print(n)
for r in M: print(fmt(r))
