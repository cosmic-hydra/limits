import sys
L=sys.stdin.read().split("\n")
def ints(i): return [int(t) for t in L[i].split()] if L[i] else []
def b(v): print("true" if v else "false")
l=ints(0); s=ints(1); b(len(s)==0 or any(l[i:i+len(s)]==s for i in range(len(l)-len(s)+1)))
