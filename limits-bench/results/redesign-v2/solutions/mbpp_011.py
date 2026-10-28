import sys
L=sys.stdin.read().split("\n")
def li(s): return [int(x) for x in s.split()]
def sh(x): print(" ".join(map(str,x)))
def b(x): print("true" if x else "false")
s=L[0];c=L[1]
i=s.find(c)
if i>=0: s=s[:i]+s[i+1:]
j=s.rfind(c)
if j>=0: s=s[:j]+s[j+1:]
print(s)
