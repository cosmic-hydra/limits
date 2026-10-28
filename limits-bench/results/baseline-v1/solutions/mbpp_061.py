import sys
L=sys.stdin.read().split("\n")
def ints(i): return [int(t) for t in L[i].split()] if L[i] else []
def b(v): print("true" if v else "false")
s=L[0]; c=0
for i in range(len(s)):
    t=0
    for j in range(i,len(s)):
        t+=ord(s[j])-48
        if t==j-i+1: c+=1
print(c)
