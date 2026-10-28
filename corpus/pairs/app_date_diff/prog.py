# Days between pairs of ISO dates "YYYY-MM-DD YYYY-MM-DD" on stdin, plus the weekday of each start.
import sys

def leap(y):
    return y % 4 == 0 and (y % 100 != 0 or y % 400 == 0)

def days_in(y, m):
    return [31, 29 if leap(y) else 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][m - 1]

def ordinal(s):
    y, m, d = map(int, s.split("-"))
    assert 1 <= m <= 12 and 1 <= d <= days_in(y, m)
    n = 0
    for yy in range(1970, y):
        n += 366 if leap(yy) else 365
    for mm in range(1, m):
        n += days_in(y, mm)
    return n + d - 1

DAYS = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"]
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    a, b = line.split(" ")
    print(a, DAYS[ordinal(a) % 7], "->", b, ordinal(b) - ordinal(a), "days")
