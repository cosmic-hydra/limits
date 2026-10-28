# Drive a traffic-light state machine with events from stdin (tick / emergency / reset).
import sys

DUR = {"red": 3, "green": 2, "yellow": 1}
NEXT = {"red": "green", "green": "yellow", "yellow": "red"}
state, left, flashing = "red", 3, False
for t, ev in enumerate([l for l in sys.stdin.read().split("\n") if l != ""]):
    if ev == "emergency":
        flashing = True
    elif ev == "reset":
        state, left, flashing = "red", DUR["red"], False
    elif ev == "tick" and not flashing:
        left -= 1
        if left == 0:
            state = NEXT[state]
            left = DUR[state]
    elif ev != "tick":
        print(t, "ignored", ev)
        continue
    print(t, "flashing" if flashing else f"{state}({left})")
