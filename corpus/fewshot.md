# Limits few-shot pairs

Eight short Python -> Limits translations from `corpus/pairs/`, each verified by
`verify.py` (same stdout as the Python program on every test input). Use them as
retrieval examples next to `native/LLM.md`. Regenerate with `python3 make_fewshot.py`.

## app_payments

Teaches: enum with payloads, exhaustive match, Option from a str match. Tokens (o200k): Python 179, Limits 265.

```python
# Price payment events from stdin: "card CENTS", "cash CENTS", "refund CENTS", "voucher CODE".
import sys

def fee(kind, cents):
    if kind == "card":
        return cents * 3 // 100
    return 0

balance = 0
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    kind, arg = line.split(" ")
    if kind in ("card", "cash"):
        net = int(arg) - fee(kind, int(arg))
    elif kind == "refund":
        net = -int(arg)
    elif kind == "voucher":
        net = 500 if arg.startswith("V") else 0
    else:
        print("unknown:", line)
        continue
    balance += net
    print(kind, net)
print("balance", balance)
```

```limits
# Price payment events from stdin: "card CENTS", "cash CENTS", "refund CENTS", "voucher CODE".
uses stdin

enum Payment:
    Card(cents: int)
    Cash(cents: int)
    Refund(cents: int)
    Voucher(code: str)

fn parse(kind: str, arg: str) -> Option[Payment]:
    match kind:
        "card": Some(Card(int(arg)))
        "cash": Some(Cash(int(arg)))
        "refund": Some(Refund(int(arg)))
        "voucher": Some(Voucher(arg))
        _: None

fn net(p: Payment) -> int:
    match p:
        Card(c): c - c * 3 / 100
        Cash(c): c
        Refund(c): -c
        Voucher(code):
            if split(code, "")[0] == "V": return 500
            0

var balance = 0
var line = env.stdin.read_line()
while line != "":
    let w = split(line, " ")
    match parse(w[0], w[1]):
        None: print("unknown:", line)
        Some(p):
            balance += net(p)
            print(w[0], net(p))
    line = env.stdin.read_line()
print("balance", balance)
```

## app_kv_parse

Teaches: Result, the ? operator, structs, any() over a comprehension. Tokens (o200k): Python 223, Limits 294.

```python
# Parse "key=int;key=int" records from stdin; print the sum per record or the first error.
import sys

def parse_record(line):
    total = 0
    keys = []
    for field in line.split(";"):
        kv = field.split("=")
        if len(kv) != 2 or kv[0] == "":
            raise ValueError("bad field '" + field + "'")
        if kv[0] in keys:
            raise ValueError("duplicate key " + kv[0])
        try:
            v = int(kv[1])
        except ValueError:
            raise ValueError("not a number: " + kv[1])
        keys.append(kv[0])
        total += v
    return keys, total

for n, line in enumerate(sys.stdin.read().split("\n"), 1):
    if line == "":
        break
    try:
        keys, total = parse_record(line)
        print(n, ",".join(sorted(keys)), total)
    except ValueError as e:
        print(n, "error:", e)
```

```limits
# Parse "key=int;key=int" records from stdin; print the sum per record or the first error.
uses stdin

struct Field:
    key: str
    value: int

fn parse_field(field: str) -> Result[Field, str]:
    let kv = split(field, "=")
    if len(kv) != 2 or kv[0] == "":
        return Err("bad field '" + field + "'")
    match parse_int(kv[1]):
        Some(v): Ok(Field { key: kv[0], value: v })
        None: Err("not a number: " + kv[1])

fn parse_record(line: str) -> Result[[Field], str]:
    var fields: [Field] = []
    for part in split(line, ";"):
        let f = parse_field(part)?
        if any([g.key == f.key for g in fields]):
            return Err("duplicate key " + f.key)
        push(fields, f)
    Ok(fields)

var n = 1
var line = env.stdin.read_line()
while line != "":
    match parse_record(line):
        Ok(fields):
            var keys = [f.key for f in fields]
            sort(keys)
            print(n, join(keys, ","), sum([f.value for f in fields]))
        Err(e): print(n, "error:", e)
    n += 1
    line = env.stdin.read_line()
```

## app_temps

Teaches: Option and ? on parse_float, float arithmetic, no implicit int->float. Tokens (o200k): Python 238, Limits 242.

```python
# Convert temperature readings like "21.5C" or "70F" from stdin; summarise in Celsius.
import sys

def parse(s):
    if len(s) < 2 or s[-1] not in "CF":
        return None
    try:
        v = float(s[:-1])
    except ValueError:
        return None
    return v if s[-1] == "C" else (v - 32.0) * 5.0 / 9.0

vals = []
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    c = parse(line)
    if c is None:
        print("bad reading:", line)
    else:
        vals.append(c)
        print(line, "=", c, "C")
if vals:
    total = 0.0
    for v in vals:  # plain left-to-right sum (sum() on floats is compensated since 3.12)
        total += v
    print("min", min(vals), "max", max(vals), "mean", total / len(vals))
else:
    print("no readings")
```

```limits
# Convert temperature readings like "21.5C" or "70F" from stdin; summarise in Celsius.
uses stdin

fn parse(s: str) -> Option[float]:
    let cs = split(s, "")
    if len(cs) < 2:
        return None
    let unit = cs[len(cs) - 1]
    let v = parse_float(join([cs[i] for i in 0..len(cs) - 1], ""))?
    match unit:
        "C": Some(v)
        "F": Some((v - 32.0) * 5.0 / 9.0)
        _: None

var vals: [float] = []
var line = env.stdin.read_line()
while line != "":
    match parse(line):
        Some(c):
            push(vals, c)
            print(line, "=", c, "C")
        None: print("bad reading:", line)
    line = env.stdin.read_line()
if len(vals) > 0:
    print("min", min(vals), "max", max(vals), "mean", sum(vals) / float(len(vals)))
else:
    print("no readings")
```

## app_caesar

Teaches: args capability, fn main(env), match on Option with early return, inline test. Tokens (o200k): Python 144, Limits 276.

```python
# Caesar-shift stdin text by the amount in argv[1] (negative decodes); keeps case and non-letters.
import sys

shift = int(sys.argv[1]) if len(sys.argv) > 1 else 13
lower = "abcdefghijklmnopqrstuvwxyz"
upper = lower.upper()
for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    out = ""
    for c in line:
        if c in lower:
            out += lower[(lower.index(c) + shift) % 26]
        elif c in upper:
            out += upper[(upper.index(c) + shift) % 26]
        else:
            out += c
    print(out)
```

```limits
# Caesar-shift stdin text by the amount in argv[1] (negative decodes); keeps case and non-letters.
uses stdin, args

fn shift_char(c: str, k: int) -> str:
    let lower = split("abcdefghijklmnopqrstuvwxyz", "")
    let upper = split("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "")
    match find(lower, c):
        Some(i): return lower[((i + k) % 26 + 26) % 26]
        None: pass
    match find(upper, c):
        Some(i): upper[((i + k) % 26 + 26) % 26]
        None: c

test "round trip":
    let s = "Hello, World!"
    let enc = join([shift_char(c, 3) for c in split(s, "")], "")
    assert(enc == "Khoor, Zruog!")
    assert(join([shift_char(c, -3) for c in split(enc, "")], "") == s)

fn main(env: Env):
    let k = match get(env.args.list(), 0):
        Some(a): int(a)
        None: 13
    var line = env.stdin.read_line()
    while line != "":
        print(join([shift_char(c, k) for c in split(line, "")], ""))
        line = env.stdin.read_line()
```

## app_data_dir

Teaches: fs.read narrowed with fs.sub, passing an Fs capability to a fn. Tokens (o200k): Python 112, Limits 135.

```python
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
```

```limits
# Sum the numbers in every file listed in data/index.txt (paths relative to data/).
uses fs.read

fn numbers(fs: Fs, name: str) -> [int]:
    [int(x) for x in split(fs.read(name), "\n") if x != ""]

fn main(env: Env):
    let data = env.fs.sub("data")
    var total = 0
    for name in split(data.read("index.txt"), "\n"):
        if name == "": continue
        let nums = numbers(data, name)
        print(name, len(nums), sum(nums))
        total += sum(nums)
    print("total", total)
```

## app_env_greeting

Teaches: env vars capability, match as a value with a block arm. Tokens (o200k): Python 129, Limits 170.

```python
# Greet the user named by $GREET_NAME (or "stranger"), in the language from $GREET_LANG.
import os

name = os.environ.get("GREET_NAME", "")
lang = os.environ.get("GREET_LANG", "")
greetings = {"en": "Hello", "fr": "Bonjour", "es": "Hola", "de": "Hallo"}
word = greetings.get(lang, "Hello")
if lang != "" and lang not in greetings:
    print("unknown language", lang + ", using en")
print(word + ", " + (name if name != "" else "stranger") + "!")
```

```limits
# Greet the user named by $GREET_NAME (or "stranger"), in the language from $GREET_LANG.
uses env

fn greeting(lang: str) -> Option[str]:
    match lang:
        "en": Some("Hello")
        "fr": Some("Bonjour")
        "es": Some("Hola")
        "de": Some("Hallo")
        _: None

fn main(env: Env):
    var name = env.vars.get("GREET_NAME")
    let lang = env.vars.get("GREET_LANG")
    if name == "": name = "stranger"
    let word = match greeting(lang):
        Some(w): w
        None:
            if lang != "": print("unknown language", lang + ", using en")
            "Hello"
    print(word + ", " + name + "!")
```

## app_shuffle

Teaches: inout scalars and lists, requires contract, nested loops building a list. Tokens (o200k): Python 282, Limits 343.

```python
# Deal 4 hands of 5 cards from a deck shuffled with a seeded LCG (Fisher-Yates), then score high cards.
seed = [2024]

def rnd(n):
    seed[0] = (seed[0] * 1103515245 + 12345) % 2147483648
    return (seed[0] // 65536) % n

def shuffle(deck):
    for i in range(len(deck) - 1, 0, -1):
        j = rnd(i + 1)
        deck[i], deck[j] = deck[j], deck[i]

ranks = ["2", "3", "4", "5", "6", "7", "8", "9", "T", "J", "Q", "K", "A"]
suits = ["c", "d", "h", "s"]
deck = [r + s for s in suits for r in ranks]
shuffle(deck)
for h in range(4):
    hand = deck[h * 5:(h + 1) * 5]
    points = sum(max(0, ranks.index(c[0]) - 8) for c in hand)
    print("hand", h + 1, " ".join(hand), "points", points)
print("left", len(deck) - 20)
```

```limits
# Deal 4 hands of 5 cards from a deck shuffled with a seeded LCG (Fisher-Yates), then score high cards.
fn rnd(seed: inout int, n: int) -> int:
    requires n > 0
    seed = (seed * 1103515245 + 12345) % 2147483648
    (seed / 65536) % n

fn shuffle(deck: inout [str], seed: inout int):
    var i = len(deck) - 1
    while i > 0:
        let j = rnd(&seed, i + 1)
        let t = deck[i]
        deck[i] = deck[j]
        deck[j] = t
        i -= 1

let ranks = ["2", "3", "4", "5", "6", "7", "8", "9", "T", "J", "Q", "K", "A"]
var deck: [str] = []
for s in ["c", "d", "h", "s"]:
    for r in ranks:
        push(deck, r + s)
var seed = 2024
shuffle(&deck, &seed)
for h in 0..4:
    let hand = [deck[h * 5 + k] for k in 0..5]
    let points = sum([max(0, unwrap(find(ranks, split(c, "")[0])) - 8) for c in hand])
    print("hand", h + 1, join(hand, " "), "points", points)
print("left", len(deck) - 20)
```

## app_matrix_rotate

Teaches: inout [[int]], in-place swaps, comprehension over a range. Tokens (o200k): Python 192, Limits 270.

```python
# Read an N x N matrix from stdin and rotate it 90 degrees clockwise K times, in place.
import sys

def rotate(m):
    n = len(m)
    for i in range(n):
        for j in range(i + 1, n):
            m[i][j], m[j][i] = m[j][i], m[i][j]
    for row in m:
        row.reverse()

lines = sys.stdin.read().split("\n")
n, k = int(lines[0].split(" ")[0]), int(lines[0].split(" ")[1])
m = [[int(x) for x in lines[1 + i].split(" ")] for i in range(n)]
for _ in range(k % 4):
    rotate(m)
for row in m:
    print(" ".join(str(x) for x in row))
print("trace", sum(m[i][i] for i in range(n)))
```

```limits
# Read an N x N matrix from stdin and rotate it 90 degrees clockwise K times, in place.
uses stdin

fn rotate(m: inout [[int]]):
    let n = len(m)
    for i in 0..n:
        for j in i + 1..n:
            let t = m[i][j]
            m[i][j] = m[j][i]
            m[j][i] = t
    for i in 0..n:
        for j in 0..n / 2:
            let t = m[i][j]
            m[i][j] = m[i][n - 1 - j]
            m[i][n - 1 - j] = t

let head = split(env.stdin.read_line(), " ")
let n = int(head[0])
let k = int(head[1])
var m: [[int]] = []
for i in 0..n:
    push(m, [int(x) for x in split(env.stdin.read_line(), " ")])
for r in 0..k % 4:
    rotate(&m)
for row in m:
    print(join([str(x) for x in row], " "))
print("trace", sum([m[i][i] for i in 0..n]))
```
