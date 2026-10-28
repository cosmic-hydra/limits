# Reference solution (official, unchanged) wrapped in the Limits-Bench I/O contract.

def comb_sort(nums):
    shrink_fact = 1.3
    gaps = len(nums)
    swapped = True
    i = 0
    while gaps > 1 or swapped:
        gaps = int(float(gaps) / shrink_fact)
        swapped = False
        i = 0
        while gaps + i < len(nums):
            if nums[i] > nums[i+gaps]:
                nums[i], nums[i+gaps] = nums[i+gaps], nums[i]
                swapped = True
            i += 1
    return nums


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
    _write(comb_sort(*_args), '[int]')
