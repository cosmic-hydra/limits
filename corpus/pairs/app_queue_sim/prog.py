# Simulate a single-server queue: customers "arrival service" from stdin, report waits.
import sys

custs = []
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    a, s = line.split(" ")
    custs.append((int(a), int(s)))
free_at = 0
waits = []
for i, (arr, svc) in enumerate(custs):
    start = max(arr, free_at)
    waits.append(start - arr)
    free_at = start + svc
    print(f"c{i} arrives {arr} starts {start} leaves {free_at} waited {start - arr}")
if waits:
    print("max wait", max(waits), "total wait", sum(waits), "busy until", free_at)
