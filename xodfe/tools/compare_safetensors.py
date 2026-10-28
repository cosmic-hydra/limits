"""Compare two .safetensors files tensor-by-tensor (dtype, shape, raw bytes).
Usage: python3 -I compare_safetensors.py A.safetensors B.safetensors"""
import json, struct, sys
import numpy as np

DT = {"F32": np.float32, "F16": np.float16, "BF16": np.uint16, "I8": np.int8}

def load(path):
    b = open(path, "rb").read()
    n = struct.unpack("<Q", b[:8])[0]
    hdr = json.loads(b[8:8 + n])
    meta = hdr.pop("__metadata__", {})
    data = b[8 + n:]
    out = {}
    for k, v in hdr.items():
        s, e = v["data_offsets"]
        out[k] = (v["dtype"], v["shape"], np.frombuffer(data[s:e], dtype=DT[v["dtype"]]).reshape(v["shape"]))
    return meta, out

ma, a = load(sys.argv[1])
mb, b = load(sys.argv[2])
assert ma == mb, (ma, mb)
assert a.keys() == b.keys(), set(a) ^ set(b)
for k in a:
    assert a[k][0] == b[k][0] and a[k][1] == b[k][1], k
    assert a[k][2].tobytes() == b[k][2].tobytes(), k
print(f"OK: {len(a)} tensors identical (dtype, shape, bytes); metadata {ma}")
