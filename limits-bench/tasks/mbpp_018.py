# Reference solution (official, unchanged) wrapped in the Limits-Bench I/O contract.

NO_OF_CHARS = 256
def str_to_list(string): 
	temp = [] 
	for x in string: 
		temp.append(x) 
	return temp 
def lst_to_string(List): 
	return ''.join(List) 
def get_char_count_array(string): 
	count = [0] * NO_OF_CHARS 
	for i in string: 
		count[ord(i)] += 1
	return count 
def remove_dirty_chars(string, second_string): 
	count = get_char_count_array(second_string) 
	ip_ind = 0
	res_ind = 0
	temp = '' 
	str_list = str_to_list(string) 
	while ip_ind != len(str_list): 
		temp = str_list[ip_ind] 
		if count[ord(temp)] == 0: 
			str_list[res_ind] = str_list[ip_ind] 
			res_ind += 1
		ip_ind+=1
	return lst_to_string(str_list[0:res_ind])


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
    _args = [_read(t) for t in ['str', 'str']]
    _write(remove_dirty_chars(*_args), 'str')
