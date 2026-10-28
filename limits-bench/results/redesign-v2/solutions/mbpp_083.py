import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(w) for w in s.split()]
def b(v): print("true" if v else "false")
s=sum(ord(c)-96 for c in L[0])%26; print("z" if s==0 else chr(96+s))
