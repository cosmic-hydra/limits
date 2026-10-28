import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def fmt(x): return " ".join(map(str,x))
s=L[0];n=len(s)
for k in range(1,n+1):
  if s[k:]+s[:k]==s: print(k);break
else: print(1 if n==0 else n)
