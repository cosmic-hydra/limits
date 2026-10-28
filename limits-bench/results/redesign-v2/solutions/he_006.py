s=input()
o=[]
for g in s.split():
    d=m=0
    for c in g:
        if c=='(':d+=1;m=max(m,d)
        elif c==')':d-=1
    o.append(str(m))
print(' '.join(o))
