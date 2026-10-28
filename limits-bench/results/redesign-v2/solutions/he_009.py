xs=[int(x) for x in input().split()]
o=[];m=None
for x in xs:
    m=x if m is None else max(m,x);o.append(str(m))
print(' '.join(o))
