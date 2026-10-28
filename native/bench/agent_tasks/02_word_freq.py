# Word frequency: top 10 words by count (ties by word).
from collections import Counter

S = 7
def rnd():
    global S
    S = (S * 1103515245 + 12345) % 2147483648
    return S // 65536

vocab = "the a of to and in is it you that he was for on are with as his they be at one have this from or had by hot word".split(" ")
words = []
for _ in range(300000):
    a = rnd() % len(vocab)
    b = rnd() % len(vocab)
    words.append(vocab[min(a, b)])
cnt = Counter(" ".join(words).split(" "))
for w, c in sorted(cnt.items(), key=lambda x: (-x[1], x[0]))[:10]:
    print(w, c)
