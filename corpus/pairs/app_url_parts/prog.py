# Split URLs from stdin into scheme, host, port (default by scheme) and path.
import sys

DEFAULT = {"http": 80, "https": 443}

def parse(url):
    if "://" not in url:
        return None
    scheme, rest = url.split("://", 1)
    slash = rest.find("/")
    hostport, path = (rest, "/") if slash < 0 else (rest[:slash], rest[slash:])
    if ":" in hostport:
        host, p = hostport.split(":", 1)
        if not p.isdigit():
            return None
        port = int(p)
    else:
        host = hostport
        port = DEFAULT.get(scheme)
    if host == "":
        return None
    return scheme, host, port, path

for line in sys.stdin.read().split("\n"):
    if line == "":
        break
    r = parse(line)
    if r is None:
        print("invalid:", line)
    else:
        scheme, host, port, path = r
        print(f"scheme={scheme} host={host} port={'none' if port is None else port} path={path}")
