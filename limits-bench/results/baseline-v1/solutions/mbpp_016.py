import sys
L=sys.stdin.read().split("\n")
def ints(s): return [int(x) for x in s.split()]
def fmt(x): return " ".join(map(str,x))
import re;print("true" if re.fullmatch(r"[a-z]+_[a-z]+",L[0]) else "false")
