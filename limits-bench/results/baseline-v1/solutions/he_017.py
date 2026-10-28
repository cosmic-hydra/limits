import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def strs(i):
    n=int(L[i]); return L[i+1:i+1+n], i+1+n
def ps(xs):
    print(len(xs))
    for x in xs: print(x)

m={"o":4,"o|":2,".|":1}
print(" ".join(str(m[x]) for x in L[0].split(" ") if x))
