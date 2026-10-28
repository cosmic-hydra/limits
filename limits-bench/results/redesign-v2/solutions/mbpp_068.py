import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(w) for w in s.split()]
def b(v): print("true" if v else "false")
a=ints(L[0]); b(all(a[i]<=a[i+1] for i in range(len(a)-1)) or all(a[i]>=a[i+1] for i in range(len(a)-1)))
