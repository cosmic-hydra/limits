# Reference solution (official, unchanged) wrapped in the Limits-Bench I/O contract.


def is_palindrome(string: str) -> bool:
    """ Test if given string is a palindrome """
    return string == string[::-1]


def make_palindrome(string: str) -> str:
    """ Find the shortest palindrome that begins with a supplied string.
    Algorithm idea is simple:
    - Find the longest postfix of supplied string that is a palindrome.
    - Append to the end of the string reverse of a string prefix that comes before the palindromic suffix.
    >>> make_palindrome('')
    ''
    >>> make_palindrome('cat')
    'catac'
    >>> make_palindrome('cata')
    'catac'
    """
    if not string:
        return ''

    beginning_of_suffix = 0

    while not is_palindrome(string[beginning_of_suffix:]):
        beginning_of_suffix += 1

    return string + string[:beginning_of_suffix][::-1]


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
    _args = [_read(t) for t in ['str']]
    _write(make_palindrome(*_args), 'str')
