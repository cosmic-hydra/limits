import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def fmt(x): return " ".join(map(str,x))
n=int(L[0]);f=[0,0,2,0]
for i in range(4,n+1): f.append(f[-1]+f[-2]+f[-3]+f[-4])
print(f[n])
