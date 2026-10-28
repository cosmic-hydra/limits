"""Check that an exported stories .safetensors holds exactly the llama2.c .bin weights.
Usage: python3 -I check_llama2c.py model.bin exported.safetensors"""
import json, struct, sys
import numpy as np
raw = open(sys.argv[1], "rb").read()
d, h, L, nh, nkv, v, seq = struct.unpack("<7i", raw[:28])
w = np.frombuffer(raw[28:], dtype=np.float32)
b = open(sys.argv[2], "rb").read(); n = struct.unpack("<Q", b[:8])[0]
hdr = json.loads(b[8:8 + n]); data = b[8 + n:]
def t(name):
    s, e = hdr[name]["data_offsets"]; return np.frombuffer(data[s:e], dtype=np.float32)
kv = nkv * (d // nh); p = 0
def take(k):
    global p; r = w[p:p + k]; p += k; return r
groups = [("tok_embeddings.weight", v * d, False)] + [(f, sz, True) for f, sz in [
    ("attention_norm", d), ("attention.wq", d * d), ("attention.wk", kv * d), ("attention.wv", kv * d),
    ("attention.wo", d * d), ("ffn_norm", d), ("feed_forward.w1", h * d), ("feed_forward.w2", d * h),
    ("feed_forward.w3", h * d)]] + [("norm.weight", d, False)]
for name, sz, per_layer in groups:
    if per_layer:
        blk = take(L * sz)
        for i in range(L):
            assert np.array_equal(blk[i*sz:(i+1)*sz].view(np.uint32), t(f"layers.{i}.{name}.weight").view(np.uint32)), (name, i)
    else:
        assert np.array_equal(take(sz).view(np.uint32), t(name).view(np.uint32)), name
print(f"OK: all {len(hdr) - ('__metadata__' in hdr)} tensors match the llama2.c checkpoint bit-for-bit")
