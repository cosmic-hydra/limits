import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(w) for w in s.split()]
def b(v): print("true" if v else "false")
s=L[0]; d=[int(c) if c.isdigit() else 0 for c in s]; c=0
for i in range(len(d)):
    t=0
    for j in range(i,len(d)):
        t+=d[j]; c+= t==j-i+1
print(c)
