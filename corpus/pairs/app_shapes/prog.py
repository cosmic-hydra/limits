# Parse shapes from stdin ("circle R", "rect W H", "tri A B C") and report areas.
import sys, math

def area(kind, nums):
    if kind == "circle":
        return 3.14159 * nums[0] * nums[0]
    if kind == "rect":
        return nums[0] * nums[1]
    a, b, c = nums
    s = (a + b + c) / 2.0
    return math.sqrt(s * (s - a) * (s - b) * (s - c))

total = 0.0
largest = None
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    p = line.split(" ")
    want = {"circle": 1, "rect": 2, "tri": 3}.get(p[0])
    if want is None or len(p) - 1 != want:
        print("skip:", line)
        continue
    a = area(p[0], [float(x) for x in p[1:]])
    total += a
    if largest is None or a > largest[1]:
        largest = (p[0], a)
    print(p[0], a)
print("total", total)
if largest is not None:
    print("largest", largest[0])
