# Summary statistics of the integers on stdin, using a helper module.
import sys
from util import stats

xs = [int(w) for w in sys.stdin.read().split() if w != ""]
if not xs:
    print("no data")
else:
    m = stats.mean100(xs)
    md = stats.median2(xs)
    print("n", len(xs))
    print("mean", f"{m // 100}.{m % 100:02d}" if m >= 0 else "negative")
    print("median", md // 2 if md % 2 == 0 else str(md // 2) + ".5")
    print("mode", stats.mode(xs))
    print("range", min(xs), max(xs))
