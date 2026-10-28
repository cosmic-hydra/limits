import sys
L=sys.stdin.read().split("\n")
def li(s): return [int(x) for x in s.split()]
def sh(x): print(" ".join(map(str,x)))
def b(x): print("true" if x else "false")
s=L[0];n=len(s);r=n
for i in range(1,n+1):
    if (s+s)[i:i+n]==s: r=i;break
print(r)
