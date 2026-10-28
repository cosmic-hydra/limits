# Read KEY=VALUE config from a file named on the command line; report typed settings.
import sys

def parse(text):
    cfg = []
    for n, line in enumerate(text.split("\n"), 1):
        if line == "" or line.startswith("#"):
            continue
        if "=" not in line:
            return None, "line " + str(n) + ": missing '='"
        k, v = line.split("=", 1)
        cfg.append((k, v))
    return cfg, None

def lookup(cfg, key):
    for k, v in cfg:
        if k == key:
            return v
    return None

def get_int(cfg, key, default):
    v = lookup(cfg, key)
    if v is None:
        return default
    try:
        return int(v)
    except ValueError:
        return default

path = sys.argv[1]
cfg, err = parse(open(path).read())
if err is not None:
    print("error:", err)
else:
    print("name:", lookup(cfg, "name") or "(unset)")
    print("port:", get_int(cfg, "port", 8080))
    print("workers:", get_int(cfg, "workers", 1))
    print("keys:", len(cfg))
