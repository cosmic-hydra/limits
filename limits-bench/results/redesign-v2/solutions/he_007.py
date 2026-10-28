import sys
L=sys.stdin.read().split('\n')
n=int(L[0]);xs=L[1:1+n];sub=L[1+n] if len(L)>1+n else ''
r=[x for x in xs if sub in x]
print(len(r))
for x in r:print(x)
