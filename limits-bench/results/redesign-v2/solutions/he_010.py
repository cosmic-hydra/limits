s=input()
k=0
while k<len(s) and s[k:]!=s[k:][::-1]:k+=1
print(s+s[:k][::-1])
