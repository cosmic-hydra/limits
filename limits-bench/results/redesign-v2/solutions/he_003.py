import sys
xs=[int(x) for x in sys.stdin.readline().split()]
b=0;r=False
for x in xs:
    b+=x
    if b<0:r=True
print('true' if r else 'false')
