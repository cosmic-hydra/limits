import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def fmt(x): return " ".join(map(str,x))
n=int(L[0])
def ip(p):
  if p<2: return False
  d=2
  while d*d<=p:
    if p%d==0: return False
    d+=1
  return True
a,b=0,1
while True:
  a,b=b,a+b
  if ip(a):
    n-=1
    if n==0: print(a);break
