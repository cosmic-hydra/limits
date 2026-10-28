//! Import of karpathy/llama2.c checkpoints (`.bin`) and `tokenizer.bin`.

use crate::error::{fail, Result};
use crate::format::{Value, Writer};

/// llama2.c `Config` (7 x i32 at the start of the checkpoint).
#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub dim: u64,
    pub hidden_dim: u64,
    pub n_layers: u64,
    pub n_heads: u64,
    pub n_kv_heads: u64,
    pub vocab_size: u64,
    pub seq_len: u64,
    pub shared_classifier: bool,
}

fn i32_at(b: &[u8], i: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

pub fn read_config(b: &[u8]) -> Result<Config> {
    let mut v = [0i32; 7];
    for (i, x) in v.iter_mut().enumerate() {
        *x = i32_at(b, i * 4).map_or_else(|| fail("llama2.c: file too short for config"), Ok)?;
    }
    let shared = v[5] > 0;
    let u = |x: i32| x.unsigned_abs() as u64;
    let c = Config {
        dim: u(v[0]),
        hidden_dim: u(v[1]),
        n_layers: u(v[2]),
        n_heads: u(v[3]),
        n_kv_heads: u(v[4]),
        vocab_size: u(v[5]),
        seq_len: u(v[6]),
        shared_classifier: shared,
    };
    if [
        c.dim,
        c.hidden_dim,
        c.n_layers,
        c.n_heads,
        c.n_kv_heads,
        c.vocab_size,
        c.seq_len,
    ]
    .contains(&0)
        || !c.dim.is_multiple_of(c.n_heads)
        || !c.n_heads.is_multiple_of(c.n_kv_heads)
        || c.dim > 1 << 16
        || c.n_layers > 1024
    {
        return fail(format!("llama2.c: implausible config {v:?}"));
    }
    Ok(c)
}

/// Convert a llama2.c checkpoint (+ optional tokenizer.bin bytes) to XODFe.
/// Weight matrices keep llama2.c's row-major `[out, in]` layout.
pub fn import(ckpt: &[u8], tokenizer: Option<&[u8]>) -> Result<Writer> {
    let c = read_config(ckpt)?;
    let (d, h, l, v) = (c.dim, c.hidden_dim, c.n_layers, c.vocab_size);
    let head = d / c.n_heads;
    let kv = c.n_kv_heads * head;
    let mut pos = 28usize;
    let mut take = |n: u64| -> Result<Vec<u8>> {
        let bytes = n.checked_mul(4).and_then(|b| usize::try_from(b).ok());
        let s = bytes.and_then(|b| ckpt.get(pos..pos.checked_add(b)?));
        let Some(s) = s else {
            return fail("llama2.c: checkpoint truncated");
        };
        pos += s.len();
        Ok(s.to_vec())
    };
    let mut w = Writer::new();
    let tok = take(v * d)?;
    let att_norm = take(l * d)?;
    let wq = take(l * d * d)?;
    let wk = take(l * kv * d)?;
    let wv = take(l * kv * d)?;
    let wo = take(l * d * d)?;
    let ffn_norm = take(l * d)?;
    let w1 = take(l * h * d)?;
    let w2 = take(l * d * h)?;
    let w3 = take(l * h * d)?;
    let norm = take(d)?;
    take(c.seq_len * head)?; // freq_cis_real + freq_cis_imag (unused: RoPE is recomputed)
    let wcls = if c.shared_classifier {
        None
    } else {
        Some(take(v * d)?)
    };
    let layer =
        |all: &[u8], i: u64, n: u64| all[(i * n * 4) as usize..((i + 1) * n * 4) as usize].to_vec();
    w.tensor(
        "tok_embeddings.weight",
        crate::format::DType::F32,
        &[v, d],
        tok,
    )?;
    for i in 0..l {
        let p = format!("layers.{i}.");
        let f32t = crate::format::DType::F32;
        w.tensor(
            &format!("{p}attention_norm.weight"),
            f32t,
            &[d],
            layer(&att_norm, i, d),
        )?;
        w.tensor(
            &format!("{p}attention.wq.weight"),
            f32t,
            &[d, d],
            layer(&wq, i, d * d),
        )?;
        w.tensor(
            &format!("{p}attention.wk.weight"),
            f32t,
            &[kv, d],
            layer(&wk, i, kv * d),
        )?;
        w.tensor(
            &format!("{p}attention.wv.weight"),
            f32t,
            &[kv, d],
            layer(&wv, i, kv * d),
        )?;
        w.tensor(
            &format!("{p}attention.wo.weight"),
            f32t,
            &[d, d],
            layer(&wo, i, d * d),
        )?;
        w.tensor(
            &format!("{p}ffn_norm.weight"),
            f32t,
            &[d],
            layer(&ffn_norm, i, d),
        )?;
        w.tensor(
            &format!("{p}feed_forward.w1.weight"),
            f32t,
            &[h, d],
            layer(&w1, i, h * d),
        )?;
        w.tensor(
            &format!("{p}feed_forward.w2.weight"),
            f32t,
            &[d, h],
            layer(&w2, i, d * h),
        )?;
        w.tensor(
            &format!("{p}feed_forward.w3.weight"),
            f32t,
            &[h, d],
            layer(&w3, i, h * d),
        )?;
    }
    w.tensor("norm.weight", crate::format::DType::F32, &[d], norm)?;
    if let Some(wc) = wcls {
        w.tensor("output.weight", crate::format::DType::F32, &[v, d], wc)?;
    }
    if pos != ckpt.len() {
        return fail(format!(
            "llama2.c: {} unexpected trailing bytes",
            ckpt.len() - pos
        ));
    }
    for (k, x) in [
        ("llama.dim", d),
        ("llama.hidden_dim", h),
        ("llama.n_layers", l),
        ("llama.n_heads", c.n_heads),
        ("llama.n_kv_heads", c.n_kv_heads),
        ("llama.vocab_size", v),
        ("llama.seq_len", c.seq_len),
    ] {
        w.meta(k, Value::U64(x));
    }
    w.meta("general.architecture", Value::Str("llama".into()));
    w.meta("general.source", Value::Str("llama2.c checkpoint".into()));
    w.meta("llama.shared_classifier", Value::Bool(c.shared_classifier));
    w.meta("llama.rope_theta", Value::F64(10000.0));
    w.meta("llama.norm_eps", Value::F64(1e-5));
    if let Some(t) = tokenizer {
        let (max_len, tokens, scores) = read_tokenizer(t, v)?;
        w.meta("tokenizer.model", Value::Str("llama2c-bpe".into()));
        w.meta("tokenizer.max_token_length", Value::U64(max_len));
        w.meta("tokenizer.tokens", Value::Strs(tokens));
        w.meta("tokenizer.scores", Value::F32s(scores));
        w.meta("tokenizer.bos_id", Value::U64(1));
        w.meta("tokenizer.eos_id", Value::U64(2));
    }
    Ok(w)
}

/// Parse llama2.c `tokenizer.bin`: i32 max_len, then per token f32 score, i32 len, bytes.
pub fn read_tokenizer(b: &[u8], vocab: u64) -> Result<(u64, Vec<String>, Vec<f32>)> {
    let short = || fail("tokenizer.bin: truncated");
    let Some(max_len) = i32_at(b, 0) else {
        return short();
    };
    let (mut p, mut toks, mut scores) = (4usize, Vec::new(), Vec::new());
    for i in 0..vocab {
        let (Some(s), Some(n)) = (b.get(p..p + 4), i32_at(b, p + 4)) else {
            return short();
        };
        let Some(bytes) = usize::try_from(n)
            .ok()
            .and_then(|n| b.get(p + 8..p + 8 + n))
        else {
            return short();
        };
        scores.push(f32::from_le_bytes(s.try_into().unwrap()));
        let Ok(t) = std::str::from_utf8(bytes) else {
            return fail(format!("tokenizer.bin: token {i} is not UTF-8"));
        };
        toks.push(t.to_string());
        p += 8 + bytes.len();
    }
    if p != b.len() {
        return fail("tokenizer.bin: vocab size does not match the checkpoint");
    }
    Ok((max_len.max(0) as u64, toks, scores))
}
