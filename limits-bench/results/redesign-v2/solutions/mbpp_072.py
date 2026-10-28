import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(w) for w in s.split()]
def b(v): print("true" if v else "false")
b(int(L[0])%4!=2)
