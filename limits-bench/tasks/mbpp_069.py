# Reference solution (official, unchanged) wrapped in the Limits-Bench I/O contract.

def is_sublist(l, s):
	sub_set = False
	if s == []:
		sub_set = True
	elif s == l:
		sub_set = True
	elif len(s) > len(l):
		sub_set = False
	else:
		for i in range(len(l)):
			if l[i] == s[0]:
				n = 1
				while (n < len(s)) and (l[i+n] == s[n]):
					n += 1				
				if n == len(s):
					sub_set = True
	return sub_set


import sys as _sys
_lines = _sys.stdin.read().split("\n")
_pos = 0
def _line():
    global _pos
    s = _lines[_pos] if _pos < len(_lines) else ""
    _pos += 1
    return s
def _ints(s):
    return [int(x) for x in s.split(" ")] if s != "" else []
def _read(t):
    if t == "int": return int(_line())
    if t == "bool": return _line() == "true"
    if t == "str": return _line()
    if t == "[int]": return _ints(_line())
    if t == "[str]":
        n = int(_line()); return [_line() for _ in range(n)]
    if t == "[[int]]":
        n = int(_line()); return [_ints(_line()) for _ in range(n)]
    raise ValueError(t)
def _write(v, t):
    if t == "bool": print("true" if v else "false")
    elif t in ("int", "str"): print(v)
    elif t == "[int]": print(" ".join(str(x) for x in v))
    elif t == "[str]":
        print(len(v))
        for x in v: print(x)
    elif t == "[[int]]":
        print(len(v))
        for r in v: print(" ".join(str(x) for x in r))
    else: raise ValueError(t)

if __name__ == '__main__':
    _args = [_read(t) for t in ['[int]', '[int]']]
    _write(is_sublist(*_args), 'bool')
