s=input()
res=[];cur='';d=0
for c in s:
    if c=='(':d+=1;cur+=c
    elif c==')':
        d-=1;cur+=c
        if d==0:res.append(cur);cur=''
print(len(res))
for r in res:print(r)
