import sys
L=sys.stdin.read().split("\n")
def ints(i): return [int(t) for t in L[i].split()] if L[i] else []
def b(v): print("true" if v else "false")
t=sum(ord(c)-96 for c in L[0])
print("z" if t%26==0 else chr(96+t%26))
