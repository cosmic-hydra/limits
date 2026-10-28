import sys
L=sys.stdin.read().split("\n")
def li(s): return [int(x) for x in s.split()]
def sh(x): print(" ".join(map(str,x)))
def b(x): print("true" if x else "false")
m=int(L[0]);r=[li(L[1+i]) for i in range(m)];r.sort(key=sum);print(m)
for x in r: sh(x)
