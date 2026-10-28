# Sum the numbers in every file listed in data/index.txt (paths relative to data/).
import sys, os

def read(name):
    return open(os.path.join("data", name)).read()

total = 0
for name in read("index.txt").split("\n"):
    if name == "":
        continue
    nums = [int(x) for x in read(name).split("\n") if x.strip() != ""]
    print(name, len(nums), sum(nums))
    total += sum(nums)
print("total", total)
