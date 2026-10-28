# FizzBuzz variant with 3/5/7.
lines = []
words = 0
for i in range(1, 500001):
    s = "".join(w for d, w in ((3, "Fizz"), (5, "Buzz"), (7, "Bazz")) if i % d == 0)
    words += bool(s)
    lines.append(s or str(i))
print(words, len("\n".join(lines)), lines[104])
