import sys
L=sys.stdin.read().split('\n');s=L[0];t=L[1] if len(L)>1 else ''
print(sum(1 for i in range(len(s)-len(t)+1) if s[i:i+len(t)]==t))
