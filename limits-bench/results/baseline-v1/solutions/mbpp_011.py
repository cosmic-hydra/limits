import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def fmt(x): return " ".join(map(str,x))
s=L[0];c=L[1]
i=s.find(c)
if c and i>=0:
  s=s[:i]+s[i+1:]
  j=s.rfind(c)
  if j>=0: s=s[:j]+s[j+1:]
print(s)
