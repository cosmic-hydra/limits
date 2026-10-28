# Reference solution (official, unchanged) wrapped in the Limits-Bench I/O contract.


def max_element(l: list):
    """Return maximum element in the list.
    >>> max_element([1, 2, 3])
    3
    >>> max_element([5, 3, -5, 2, -3, 3, 9, 0, 123, 1, -10])
    123
    """
    m = l[0]
    for e in l:
        if e > m:
            m = e
    return m


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
    _args = [_read(t) for t in ['[int]']]
    _write(max_element(*_args), 'int')
