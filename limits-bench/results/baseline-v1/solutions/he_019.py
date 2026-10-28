import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def strs(i):
    n=int(L[i]); return L[i+1:i+1+n], i+1+n
def ps(xs):
    print(len(xs))
    for x in xs: print(x)

n=["zero","one","two","three","four","five","six","seven","eight","nine"]
print(" ".join(sorted([x for x in L[0].split(" ") if x],key=n.index)))
