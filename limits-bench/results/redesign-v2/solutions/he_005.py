import sys
xs=sys.stdin.readline().split();d=sys.stdin.readline().strip()
o=[]
for i,x in enumerate(xs):
    if i:o.append(d)
    o.append(x)
print(' '.join(o))
