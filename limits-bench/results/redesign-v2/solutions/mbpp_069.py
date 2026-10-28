import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(w) for w in s.split()]
def b(v): print("true" if v else "false")
l=ints(L[0]); s=ints(L[1]); b(any(l[i:i+len(s)]==s for i in range(len(l)-len(s)+1)))
