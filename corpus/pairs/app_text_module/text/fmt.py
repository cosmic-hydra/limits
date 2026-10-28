def pad_right(s, w):
    return s + " " * max(0, w - len(s))

def pad_left(s, w):
    return " " * max(0, w - len(s)) + s

def rule(w):
    return "-" * w
