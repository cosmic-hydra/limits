"""Independent numpy re-implementation of llama2.c run.c greedy generation.
Usage: python3 -I ref_llama2c.py model.bin tokenizer.bin "prompt" [steps]"""
import struct, sys
import numpy as np
raw = open(sys.argv[1], "rb").read()
d, h, L, nh, nkv, V, seq = struct.unpack("<7i", raw[:28]); shared = V > 0; V = abs(V)
w = np.frombuffer(raw[28:], dtype=np.float32); p = 0; hs = d // nh; kv = nkv * hs
def take(*shape):
    global p; n = int(np.prod(shape)); r = w[p:p + n].reshape(shape); p += n; return r
emb = take(V, d); an = take(L, d); wq = take(L, d, d); wk = take(L, kv, d); wv = take(L, kv, d)
wo = take(L, d, d); fn = take(L, d); w1 = take(L, h, d); w2 = take(L, d, h); w3 = take(L, h, d)
nrm = take(d); take(seq * hs); cls = emb if shared else take(V, d)
t = open(sys.argv[2], "rb").read(); q = 4; vocab = []; scores = []
for _ in range(V):
    s, n = struct.unpack("<fi", t[q:q + 8]); vocab.append(t[q + 8:q + 8 + n]); scores.append(s); q += 8 + n
idx = {b: i for i, b in enumerate(vocab)}
def encode(text):
    toks = [1] + ([idx[b" "]] if text else [])
    for ch in text:
        b = ch.encode(); toks += [idx[b]] if b in idx else [x + 3 for x in b]
    while True:
        best = None
        for i in range(len(toks) - 1):
            j = idx.get(vocab[toks[i]] + vocab[toks[i + 1]])
            if j is not None and (best is None or scores[j] > best[0]): best = (scores[j], j, i)
        if best is None: return toks
        toks[best[2]] = best[1]; del toks[best[2] + 1]
def rms(x, g):
    return g * (x / np.sqrt(np.float32((x * x).mean() + 1e-5)))
kc = np.zeros((L, seq, kv), np.float32); vc = np.zeros((L, seq, kv), np.float32)
fi = np.arange(0, hs, 2, dtype=np.float32)
freq = (1.0 / np.power(np.float32(10000.0), fi / np.float32(hs))).astype(np.float32)
def rope(v, pos):
    v = v.reshape(-1, hs // 2, 2); a = (pos * freq).astype(np.float32); c, s = np.cos(a), np.sin(a)
    return np.stack([v[..., 0] * c - v[..., 1] * s, v[..., 0] * s + v[..., 1] * c], -1).reshape(-1)
def forward(tok, pos):
    x = emb[tok].copy()
    for l in range(L):
        xb = rms(x, an[l]); qv = rope(wq[l] @ xb, pos)
        kc[l, pos] = rope(wk[l] @ xb, pos); vc[l, pos] = wv[l] @ xb
        out = np.zeros(d, np.float32)
        for hh in range(nh):
            g = hh // (nh // nkv); K = kc[l, :pos + 1, g * hs:(g + 1) * hs]
            a = K @ qv[hh * hs:(hh + 1) * hs] / np.sqrt(np.float32(hs)); a = np.exp(a - a.max()); a /= a.sum()
            out[hh * hs:(hh + 1) * hs] = a @ vc[l, :pos + 1, g * hs:(g + 1) * hs]
        x = x + wo[l] @ out; xb = rms(x, fn[l]); a = w1[l] @ xb
        x = x + w2[l] @ (a / (1 + np.exp(-a)) * (w3[l] @ xb))
    return cls @ rms(x, nrm)
def piece(prev, tok):
    s = vocab[tok]
    if prev == 1 and s.startswith(b" "): s = s[1:]
    if len(s) == 6 and s.startswith(b"<0x") and s.endswith(b">"):
        b = int(s[3:5], 16); s = bytes([b]) if 32 <= b <= 126 or 9 <= b <= 13 else b""
    return s
pt = encode(sys.argv[3]); steps = min(int(sys.argv[4]) if len(sys.argv) > 4 else 256, seq)
tok, out = pt[0], b""
for pos in range(steps):
    logits = forward(tok, pos)
    nxt = pt[pos + 1] if pos + 1 < len(pt) else int(np.argmax(logits))
    if nxt == 1: break
    out += piece(tok, nxt); tok = nxt
sys.stdout.buffer.write(out + b"\n")
