//! Llama-2 inference straight from a `.xodfe` file: config and tokenizer come
//! from metadata, weights from tensors. f32/f16/bf16 matrices run in f32;
//! q8_0/q4_0 matrices stay quantized and are used directly by the matmul.
//! Semantics mirror karpathy/llama2.c `run.c` so greedy output can be compared.

use crate::error::{fail, Result};
use crate::format::{DType, Model, TensorInfo, Value};
use crate::quant::{matmul, QVec, Weights};

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub dim: usize,
    pub hidden_dim: usize,
    pub n_layers: usize,
    pub n_heads: usize,
    pub n_kv_heads: usize,
    pub vocab_size: usize,
    pub seq_len: usize,
    pub rope_theta: f32,
    pub norm_eps: f32,
}

struct Layer {
    att_norm: Vec<f32>,
    wq: Weights,
    wk: Weights,
    wv: Weights,
    wo: Weights,
    ffn_norm: Vec<f32>,
    w1: Weights,
    w2: Weights,
    w3: Weights,
}

pub struct Transformer {
    pub cfg: Config,
    tok_emb: Weights,
    layers: Vec<Layer>,
    norm: Vec<f32>,
    /// `None` = shared with `tok_emb`.
    output: Option<Weights>,
    xq: QVec,
    threads: usize,
    // run state
    x: Vec<f32>,
    xb: Vec<f32>,
    xb2: Vec<f32>,
    hb: Vec<f32>,
    hb2: Vec<f32>,
    q: Vec<f32>,
    att: Vec<f32>,
    key_cache: Vec<f32>,
    value_cache: Vec<f32>,
    pub logits: Vec<f32>,
}

fn meta_f32(m: &Model, key: &str, default: f32) -> f32 {
    match m.meta(key) {
        Some(Value::F64(v)) => *v as f32,
        _ => default,
    }
}

/// A zeroed f32 buffer; allocation failure is an error, not an abort.
fn zeros(n: usize) -> Result<Vec<f32>> {
    let mut v = Vec::new();
    if v.try_reserve_exact(n).is_err() {
        return fail(format!("run: cannot allocate {n} f32 values for run state"));
    }
    v.resize(n, 0.0);
    Ok(v)
}

impl Transformer {
    /// Load weights and allocate run state for at most `ctx` positions
    /// (clamped to the model's `llama.seq_len`), so buffers are sized by what
    /// the caller will actually generate rather than by file metadata alone.
    pub fn load(m: &Model, threads: usize, ctx: usize) -> Result<Transformer> {
        if m.meta_str("general.architecture") != Some("llama") {
            return fail("run: general.architecture must be \"llama\"");
        }
        let u = |k: &str| -> Result<usize> {
            let v = m.meta_u64(k)?;
            match usize::try_from(v) {
                Ok(v) if v > 0 && v <= 1 << 24 => Ok(v),
                _ => fail(format!("run: implausible {k} = {v}")),
            }
        };
        let mut cfg = Config {
            dim: u("llama.dim")?,
            hidden_dim: u("llama.hidden_dim")?,
            n_layers: u("llama.n_layers")?,
            n_heads: u("llama.n_heads")?,
            n_kv_heads: u("llama.n_kv_heads")?,
            vocab_size: u("llama.vocab_size")?,
            seq_len: u("llama.seq_len")?,
            rope_theta: meta_f32(m, "llama.rope_theta", 10000.0),
            norm_eps: meta_f32(m, "llama.norm_eps", 1e-5),
        };
        cfg.seq_len = cfg.seq_len.min(ctx.max(1));
        let (d, h) = (cfg.dim, cfg.hidden_dim);
        if !d.is_multiple_of(cfg.n_heads)
            || !cfg.n_heads.is_multiple_of(cfg.n_kv_heads)
            || !(d / cfg.n_heads).is_multiple_of(2)
        {
            return fail("run: dim/n_heads/n_kv_heads are inconsistent");
        }
        let kv = d / cfg.n_heads * cfg.n_kv_heads;
        // Fetch a tensor, checking its shape against the config.
        let get_t = |name: &str, shape: &[usize]| -> Result<&TensorInfo> {
            let Some(t) = m.tensor(name) else {
                return fail(format!("run: missing tensor {name:?}"));
            };
            if t.dims.iter().map(|&x| x as usize).ne(shape.iter().copied()) {
                return fail(format!(
                    "run: tensor {name:?} has shape {:?}, expected {shape:?}",
                    t.dims
                ));
            }
            Ok(t)
        };
        let get =
            |name: &str, shape: &[usize]| -> Result<Vec<f32>> { Ok(m.to_f32(get_t(name, shape)?)) };
        // Matrices keep q8_0/q4_0 codes as-is; other dtypes become f32.
        let mat = |name: &str, shape: &[usize]| -> Result<Weights> {
            let t = get_t(name, shape)?;
            Ok(match t.dtype {
                DType::Q8_0 => Weights::from_q8_0(m.data(t)),
                DType::Q4_0 => Weights::from_q4_0(m.data(t)),
                _ => Weights::F32(m.to_f32(t)),
            })
        };
        let mut layers = Vec::with_capacity(cfg.n_layers);
        for i in 0..cfg.n_layers {
            let p = |s: &str| format!("layers.{i}.{s}.weight");
            layers.push(Layer {
                att_norm: get(&p("attention_norm"), &[d])?,
                wq: mat(&p("attention.wq"), &[d, d])?,
                wk: mat(&p("attention.wk"), &[kv, d])?,
                wv: mat(&p("attention.wv"), &[kv, d])?,
                wo: mat(&p("attention.wo"), &[d, d])?,
                ffn_norm: get(&p("ffn_norm"), &[d])?,
                w1: mat(&p("feed_forward.w1"), &[h, d])?,
                w2: mat(&p("feed_forward.w2"), &[d, h])?,
                w3: mat(&p("feed_forward.w3"), &[h, d])?,
            });
        }
        let output = match m.tensor("output.weight") {
            Some(_) => Some(mat("output.weight", &[cfg.vocab_size, d])?),
            None => None,
        };
        let cache = cfg.n_layers.checked_mul(cfg.seq_len * kv);
        let att = cfg.n_heads.checked_mul(cfg.seq_len);
        let (Some(cache), Some(att)) = (cache, att) else {
            return fail("run: KV cache size overflows");
        };
        Ok(Transformer {
            tok_emb: mat("tok_embeddings.weight", &[cfg.vocab_size, d])?,
            norm: get("norm.weight", &[d])?,
            layers,
            output,
            threads: threads.max(1),
            xq: QVec::default(),
            x: vec![0.0; d],
            xb: vec![0.0; d],
            xb2: vec![0.0; d],
            hb: vec![0.0; h],
            hb2: vec![0.0; h],
            q: vec![0.0; d],
            att: zeros(att)?,
            key_cache: zeros(cache)?,
            value_cache: zeros(cache)?,
            logits: vec![0.0; cfg.vocab_size],
            cfg,
        })
    }

    /// Run one token at position `pos`; returns the logits.
    pub fn forward(&mut self, token: usize, pos: usize) -> &mut [f32] {
        let c = self.cfg;
        let (d, hs) = (c.dim, c.dim / c.n_heads);
        let kv = hs * c.n_kv_heads;
        let kv_mul = c.n_heads / c.n_kv_heads;
        let nt = self.threads;
        let xq = &mut self.xq;
        self.tok_emb.row(token, d, &mut self.x);
        for (l, w) in self.layers.iter().enumerate() {
            rmsnorm(&mut self.xb, &self.x, &w.att_norm, c.norm_eps);
            let loff = l * c.seq_len * kv;
            let kr = loff + pos * kv..loff + (pos + 1) * kv;
            matmul(&mut self.q, &self.xb, &w.wq, xq, nt);
            matmul(&mut self.key_cache[kr.clone()], &self.xb, &w.wk, xq, nt);
            matmul(&mut self.value_cache[kr.clone()], &self.xb, &w.wv, xq, nt);
            // RoPE on q (all heads) and k (kv heads).
            let k = &mut self.key_cache[kr];
            for i in (0..d).step_by(2) {
                let freq = 1.0 / c.rope_theta.powf((i % hs) as f32 / hs as f32);
                let (fci, fcr) = (pos as f32 * freq).sin_cos();
                let rot = |v: &mut [f32]| {
                    let (v0, v1) = (v[i], v[i + 1]);
                    v[i] = v0 * fcr - v1 * fci;
                    v[i + 1] = v0 * fci + v1 * fcr;
                };
                rot(&mut self.q);
                if i < kv {
                    rot(k);
                }
            }
            // Multi-head attention over the cache (GQA via kv_mul).
            let scale = (hs as f32).sqrt();
            for h in 0..c.n_heads {
                let q = &self.q[h * hs..(h + 1) * hs];
                let att = &mut self.att[h * c.seq_len..h * c.seq_len + pos + 1];
                let ko = loff + (h / kv_mul) * hs;
                for (t, a) in att.iter_mut().enumerate() {
                    let k = &self.key_cache[ko + t * kv..ko + t * kv + hs];
                    *a = dot_seq(q, k) / scale;
                }
                softmax(att);
                let xb = &mut self.xb[h * hs..(h + 1) * hs];
                xb.fill(0.0);
                for (t, &a) in att.iter().enumerate() {
                    let v = &self.value_cache[ko + t * kv..ko + t * kv + hs];
                    for (o, &vv) in xb.iter_mut().zip(v) {
                        *o += a * vv;
                    }
                }
            }
            matmul(&mut self.xb2, &self.xb, &w.wo, xq, nt);
            add(&mut self.x, &self.xb2);
            // SwiGLU FFN.
            rmsnorm(&mut self.xb, &self.x, &w.ffn_norm, c.norm_eps);
            matmul(&mut self.hb, &self.xb, &w.w1, xq, nt);
            matmul(&mut self.hb2, &self.xb, &w.w3, xq, nt);
            for (a, &b) in self.hb.iter_mut().zip(&self.hb2) {
                *a = *a * (1.0 / (1.0 + (-*a).exp())) * b;
            }
            matmul(&mut self.xb, &self.hb, &w.w2, xq, nt);
            add(&mut self.x, &self.xb);
        }
        let x = self.x.clone();
        rmsnorm(&mut self.x, &x, &self.norm, c.norm_eps);
        let cls = self.output.as_ref().unwrap_or(&self.tok_emb);
        matmul(&mut self.logits, &self.x, cls, xq, nt);
        &mut self.logits
    }
}

fn add(x: &mut [f32], y: &[f32]) {
    x.iter_mut().zip(y).for_each(|(a, b)| *a += b);
}

fn rmsnorm(o: &mut [f32], x: &[f32], w: &[f32], eps: f32) {
    let ss = x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32 + eps;
    let s = 1.0 / ss.sqrt();
    for ((o, &x), &w) in o.iter_mut().zip(x).zip(w) {
        *o = w * (s * x);
    }
}

pub fn softmax(x: &mut [f32]) {
    let max = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0;
    for v in x.iter_mut() {
        *v = (*v - max).exp();
        sum += *v;
    }
    x.iter_mut().for_each(|v| *v /= sum);
}

/// Sequential dot product (same summation order as run.c).
fn dot_seq(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).fold(0.0, |s, (x, y)| s + x * y)
}

/// llama2.c-compatible BPE tokenizer built from `tokenizer.*` metadata.
pub struct Tokenizer {
    pub vocab: Vec<String>,
    scores: Vec<f32>,
    sorted: Vec<(String, usize)>,
    pub bos: usize,
    pub eos: usize,
}

impl Tokenizer {
    pub fn from_model(m: &Model, vocab_size: usize) -> Result<Tokenizer> {
        let (Some(Value::Strs(vocab)), Some(Value::F32s(scores))) =
            (m.meta("tokenizer.tokens"), m.meta("tokenizer.scores"))
        else {
            return fail("run: file has no embedded tokenizer (tokenizer.tokens/scores)");
        };
        if vocab.len() != vocab_size || scores.len() != vocab_size || vocab_size < 259 {
            return fail("run: tokenizer size does not match llama.vocab_size");
        }
        let id = |k: &str, d: usize| m.meta_u64(k).map_or(d, |v| v as usize);
        let mut sorted: Vec<_> = vocab.iter().cloned().zip(0..).collect();
        sorted.sort();
        Ok(Tokenizer {
            vocab: vocab.clone(),
            scores: scores.clone(),
            sorted,
            bos: id("tokenizer.bos_id", 1).min(vocab_size - 1),
            eos: id("tokenizer.eos_id", 2).min(vocab_size - 1),
        })
    }

    fn lookup(&self, s: &[u8]) -> Option<usize> {
        self.sorted
            .binary_search_by(|(t, _)| t.as_bytes().cmp(s))
            .ok()
            .map(|i| self.sorted[i].1)
    }

    /// Encode with BOS and the sentencepiece dummy-prefix space, like run.c.
    pub fn encode(&self, text: &str) -> Vec<usize> {
        let mut toks = vec![self.bos];
        if !text.is_empty() {
            toks.extend(self.lookup(b" "));
        }
        let mut buf = [0u8; 4];
        for ch in text.chars() {
            let s = ch.encode_utf8(&mut buf).as_bytes();
            match self.lookup(s) {
                Some(id) => toks.push(id),
                None => toks.extend(s.iter().map(|&b| b as usize + 3)), // byte fallback
            }
        }
        loop {
            let mut best: Option<(f32, usize, usize)> = None;
            for i in 0..toks.len().saturating_sub(1) {
                let pair = [
                    self.vocab[toks[i]].as_bytes(),
                    self.vocab[toks[i + 1]].as_bytes(),
                ];
                if let Some(id) = self.lookup(&pair.concat()) {
                    if best.is_none_or(|(s, _, _)| self.scores[id] > s) {
                        best = Some((self.scores[id], id, i));
                    }
                }
            }
            let Some((_, id, i)) = best else { break };
            toks[i] = id;
            toks.remove(i + 1);
        }
        toks
    }

    /// Bytes for `token` following `prev` (strips the space after BOS, maps `<0xXX>`).
    pub fn decode(&self, prev: usize, token: usize) -> Vec<u8> {
        let mut p = self.vocab[token].as_str();
        if prev == self.bos {
            p = p.strip_prefix(' ').unwrap_or(p);
        }
        if let Some(h) = p.strip_prefix("<0x").and_then(|r| r.strip_suffix('>')) {
            if let (2, Ok(b)) = (h.len(), u8::from_str_radix(h, 16)) {
                // Like run.c's safe_printf: drop non-printable single bytes.
                let ok = (0x20..=0x7e).contains(&b) || (9..=13).contains(&b);
                return if ok { vec![b] } else { Vec::new() };
            }
        }
        p.as_bytes().to_vec()
    }
}

/// Greedy / temperature / top-p sampler with run.c's xorshift RNG.
pub struct Sampler {
    pub temperature: f32,
    pub topp: f32,
    state: u64,
}

impl Sampler {
    pub fn new(temperature: f32, topp: f32, seed: u64) -> Sampler {
        Sampler {
            temperature,
            topp,
            state: seed.max(1),
        }
    }
    fn random_f32(&mut self) -> f32 {
        let s = &mut self.state;
        *s ^= *s >> 12;
        *s ^= *s << 25;
        *s ^= *s >> 27;
        let u = (s.wrapping_mul(0x2545F4914F6CDD1D) >> 32) as u32;
        (u >> 8) as f32 / 16777216.0
    }
    pub fn sample(&mut self, logits: &mut [f32]) -> usize {
        if self.temperature == 0.0 {
            // first maximum, like run.c
            return logits
                .iter()
                .enumerate()
                .fold(
                    (0, f32::NEG_INFINITY),
                    |b, (i, &v)| if v > b.1 { (i, v) } else { b },
                )
                .0;
        }
        logits.iter_mut().for_each(|v| *v /= self.temperature);
        softmax(logits);
        let coin = self.random_f32();
        let pick = |cands: &[(f32, usize)], r: f32| {
            let mut cdf = 0.0;
            for &(p, i) in cands {
                cdf += p;
                if r < cdf {
                    return i;
                }
            }
            cands.last().map_or(0, |c| c.1)
        };
        let all = || logits.iter().copied().zip(0..);
        if self.topp <= 0.0 || self.topp >= 1.0 {
            return pick(&all().collect::<Vec<_>>(), coin);
        }
        let cutoff = (1.0 - self.topp) / (logits.len() - 1) as f32;
        let mut c: Vec<_> = all().filter(|&(p, _)| p >= cutoff).collect();
        c.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut cum = 0.0;
        let mut last = c.len();
        for (i, &(p, _)) in c.iter().enumerate() {
            cum += p;
            if cum > self.topp {
                last = i + 1;
                break;
            }
        }
        pick(&c[..last], coin * cum)
    }
}

/// Generate up to `steps` positions (prompt included, capped at seq_len),
/// calling `emit` with each decoded piece. Returns the number of forward passes
/// and seconds spent after the first one (run.c's timing convention).
pub fn generate(
    t: &mut Transformer,
    tok: &Tokenizer,
    s: &mut Sampler,
    prompt: &str,
    steps: usize,
    mut emit: impl FnMut(&[u8]),
) -> (usize, f64) {
    let prompt_toks = tok.encode(prompt);
    let steps = steps.min(t.cfg.seq_len);
    let (mut token, mut pos) = (prompt_toks[0], 0);
    let mut start = None;
    while pos < steps {
        let logits = t.forward(token, pos);
        let next = match prompt_toks.get(pos + 1) {
            Some(&p) => p,
            None => s.sample(logits),
        };
        pos += 1;
        if next == tok.bos {
            break;
        }
        emit(&tok.decode(token, next));
        token = next;
        start.get_or_insert_with(std::time::Instant::now);
    }
    (pos, start.map_or(0.0, |s| s.elapsed().as_secs_f64()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greedy_takes_first_max_and_rng_is_deterministic() {
        let mut s = Sampler::new(0.0, 0.9, 7);
        assert_eq!(s.sample(&mut [0.1, 3.0, 3.0, -1.0]), 1);
        let draw = |seed| {
            let mut s = Sampler::new(1.0, 0.9, seed);
            (0..20)
                .map(|_| s.sample(&mut [0.5, 0.2, 0.9, 0.1, 0.4]))
                .collect::<Vec<_>>()
        };
        assert_eq!(draw(42), draw(42));
        assert!(draw(42).iter().all(|&i| i < 5));
    }

    #[test]
    fn softmax_sums_to_one() {
        let mut v = [1.0, 2.0, 3.0, 1000.0];
        softmax(&mut v);
        assert!((v.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }
}
