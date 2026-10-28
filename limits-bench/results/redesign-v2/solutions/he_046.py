import sys
L=sys.stdin.read().split("\n")
def li(s): return [int(x) for x in s.split()]
def sh(x): print(" ".join(map(str,x)))
def b(x): print("true" if x else "false")
n=int(L[0]);f=[0,0,2,0]
for i in range(4,n+1): f.append(f[-1]+f[-2]+f[-3]+f[-4])
print(f[n])
