import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def fmt(x): return " ".join(map(str,x))
n=int(L[0]);print(sum(str(i).count("7") for i in range(n) if i%11==0 or i%13==0))
