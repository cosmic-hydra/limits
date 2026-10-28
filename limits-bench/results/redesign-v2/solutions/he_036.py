import sys
L=sys.stdin.read().split("\n")
def li(s): return [int(x) for x in s.split()]
def sh(x): print(" ".join(map(str,x)))
def b(x): print("true" if x else "false")
n=int(L[0]);print(sum(str(i).count("7") for i in range(max(n,0)) if i%11==0 or i%13==0))
