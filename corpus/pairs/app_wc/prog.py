# wc for the files named on the command line: lines, words, chars, then a total.
import sys

tl = tw = tc = 0
for path in sys.argv[1:]:
    text = open(path).read()
    lines = text.count("\n")
    words = len([w for w in text.replace("\n", " ").split(" ") if w != ""])
    chars = len(text)
    tl, tw, tc = tl + lines, tw + words, tc + chars
    print(lines, words, chars, path)
if len(sys.argv) > 2:
    print(tl, tw, tc, "total")
