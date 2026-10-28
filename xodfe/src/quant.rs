//! q8_0 / q4_0 quantization and the matmul kernels used by inference.
//!
//! Quantized weights are never expanded to f32: a matvec quantizes the
//! activation vector to q8_0 once, then each row is an integer dot product per
//! 32-element block scaled by `d_w * d_x` (the scheme of llama2.c `runq.c`).

use crate::num::{f16_to_f32, f32_to_f16};

pub const QK: usize = 32;

/// Quantize `x` (length a multiple of 32) to q8_0 blocks: f16 scale + 32 x i8.
pub fn quantize_q8_0(x: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(x.len() / QK * 34);
    for b in x.chunks_exact(QK) {
        let amax = b.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let h = f32_to_f16(amax / 127.0);
        let d = f16_to_f32(h);
        let id = if d == 0.0 { 0.0 } else { 1.0 / d };
        out.extend_from_slice(&h.to_le_bytes());
        out.extend(
            b.iter()
                .map(|v| (v * id).round().clamp(-127.0, 127.0) as i8 as u8),
        );
    }
    out
}

/// Quantize `x` to q4_0 blocks (ggml convention: the largest-magnitude value
/// maps to nibble 0, i.e. `d = max / -8`).
pub fn quantize_q4_0(x: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(x.len() / QK * 18);
    for b in x.chunks_exact(QK) {
        let max = b
            .iter()
            .fold(0.0f32, |m, &v| if v.abs() > m.abs() { v } else { m });
        let h = f32_to_f16(max / -8.0);
        let d = f16_to_f32(h);
        let id = if d == 0.0 { 0.0 } else { 1.0 / d };
        out.extend_from_slice(&h.to_le_bytes());
        let q = |v: f32| (v * id + 8.5).clamp(0.0, 15.0) as u8;
        out.extend((0..16).map(|j| q(b[j]) | (q(b[j + 16]) << 4)));
    }
    out
}

/// A weight matrix in its inference layout. Quantized forms keep integer
/// codes; only the per-block scales are widened to f32.
pub enum Weights {
    F32(Vec<f32>),
    Q8 { q: Vec<i8>, d: Vec<f32> },
    Q4 { q: Vec<u8>, d: Vec<f32> },
}

impl Weights {
    /// Split q8_0/q4_0 blocks into (codes, scales); `bs` = bytes per block.
    fn split(data: &[u8], bs: usize) -> (Vec<u8>, Vec<f32>) {
        let mut q = Vec::with_capacity(data.len());
        let mut d = Vec::with_capacity(data.len() / bs);
        for b in data.chunks_exact(bs) {
            d.push(f16_to_f32(u16::from_le_bytes([b[0], b[1]])));
            q.extend_from_slice(&b[2..]);
        }
        (q, d)
    }
    pub fn from_q8_0(data: &[u8]) -> Weights {
        let (q, d) = Self::split(data, 34);
        Weights::Q8 {
            q: q.into_iter().map(|b| b as i8).collect(),
            d,
        }
    }
    pub fn from_q4_0(data: &[u8]) -> Weights {
        let (q, d) = Self::split(data, 18);
        Weights::Q4 { q, d }
    }

    /// Dequantize row `r` of width `n` into `out` (used for embedding lookup).
    pub fn row(&self, r: usize, n: usize, out: &mut [f32]) {
        let nb = n / QK;
        match self {
            Weights::F32(w) => out.copy_from_slice(&w[r * n..(r + 1) * n]),
            Weights::Q8 { q, d } => {
                for (j, o) in out.iter_mut().enumerate() {
                    *o = q[r * n + j] as f32 * d[r * nb + j / QK];
                }
            }
            Weights::Q4 { q, d } => {
                for (b, o) in out.chunks_exact_mut(QK).enumerate() {
                    let (qs, s) = (&q[(r * nb + b) * 16..][..16], d[r * nb + b]);
                    for j in 0..16 {
                        o[j] = ((qs[j] & 15) as i32 - 8) as f32 * s;
                        o[j + 16] = ((qs[j] >> 4) as i32 - 8) as f32 * s;
                    }
                }
            }
        }
    }
}

/// Dot product with 8 independent accumulators so LLVM can vectorize it.
fn dot_f32(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0.0f32; 8];
    let (ac, bc) = (a.chunks_exact(8), b.chunks_exact(8));
    let tail: f32 = ac
        .remainder()
        .iter()
        .zip(bc.remainder())
        .map(|(x, y)| x * y)
        .sum();
    for (x, y) in ac.zip(bc) {
        for i in 0..8 {
            acc[i] += x[i] * y[i];
        }
    }
    acc.iter().sum::<f32>() + tail
}

/// i8 x i8 block dot, widened to i32 (vectorizes to pmaddwd-style code).
#[inline(always)]
fn dot_i8(a: &[i8; QK], b: &[i8; QK]) -> i32 {
    let mut s = 0i32;
    for i in 0..QK {
        s += a[i] as i32 * b[i] as i32;
    }
    s
}

/// Activation vector quantized to q8_0 (codes + scales).
#[derive(Default)]
pub struct QVec {
    q: Vec<i8>,
    d: Vec<f32>,
    /// Per-block sum of codes (for q4_0's offset).
    s: Vec<i32>,
}

impl QVec {
    fn set(&mut self, x: &[f32]) {
        self.q.resize(x.len(), 0);
        self.d.clear();
        self.s.clear();
        for (b, q) in x.chunks_exact(QK).zip(self.q.chunks_exact_mut(QK)) {
            let amax = b.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let d = amax / 127.0;
            let id = if d == 0.0 { 0.0 } else { 1.0 / d };
            for (o, v) in q.iter_mut().zip(b) {
                *o = (v * id).round() as i8;
            }
            self.d.push(d);
            self.s.push(q.iter().map(|&v| v as i32).sum());
        }
    }
}

fn blocks(q: &[i8]) -> impl Iterator<Item = &[i8; QK]> {
    q.chunks_exact(QK).map(|c| c.try_into().unwrap())
}

/// Compute rows rows `r0..r0+out.len()` of `w`.
fn rows(out: &mut [f32], r0: usize, x: &[f32], xq: &QVec, w: &Weights) {
    let n = x.len();
    let nb = n / QK;
    let mut u4 = vec![
        0i8;
        if matches!(w, Weights::Q4 { .. }) {
            n
        } else {
            0
        }
    ];
    for (i, o) in out.iter_mut().enumerate() {
        let r = r0 + i;
        *o = match w {
            Weights::F32(w) => dot_f32(&w[r * n..(r + 1) * n], x),
            Weights::Q8 { q, d } => {
                let (wq, wd) = (&q[r * n..(r + 1) * n], &d[r * nb..(r + 1) * nb]);
                blocks(wq)
                    .zip(blocks(&xq.q))
                    .zip(wd.iter().zip(&xq.d))
                    .map(|((a, b), (da, db))| dot_i8(a, b) as f32 * da * db)
                    .sum()
            }
            Weights::Q4 { q, d } => {
                // Unpack the row's nibbles to i8 codes (a row-sized scratch,
                // not an f32 copy); the -8 offset is folded in via sum(x).
                for (u, qs) in u4.chunks_exact_mut(QK).zip(q[r * n / 2..].chunks_exact(16)) {
                    for j in 0..16 {
                        u[j] = (qs[j] & 15) as i8;
                        u[j + 16] = (qs[j] >> 4) as i8;
                    }
                }
                blocks(&u4)
                    .zip(blocks(&xq.q))
                    .zip(d[r * nb..(r + 1) * nb].iter().zip(&xq.d).zip(&xq.s))
                    .map(|((a, b), ((da, db), sx))| (dot_i8(a, b) - 8 * sx) as f32 * da * db)
                    .sum()
            }
        };
    }
}

/// `out = W x` for row-major `W` of shape `[out.len(), x.len()]`, split by
/// rows across `threads` scoped threads when the matrix is large enough.
pub fn matmul(out: &mut [f32], x: &[f32], w: &Weights, xq: &mut QVec, threads: usize) {
    if !matches!(w, Weights::F32(_)) {
        xq.set(x);
    }
    let xq = &*xq;
    // Spawning scoped threads costs tens of microseconds each, which exceeds
    // the work in small matrices; only split ~1M+ element matmuls.
    if threads <= 1 || out.len() * x.len() < 1 << 20 {
        return rows(out, 0, x, xq, w);
    }
    // Chunks in multiples of 16 rows to keep threads on separate cache lines.
    let per = out.len().div_ceil(threads).next_multiple_of(16);
    std::thread::scope(|s| {
        for (i, o) in out.chunks_mut(per).enumerate() {
            s.spawn(move || rows(o, i * per, x, xq, w));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{decode_f32, DType};

    fn data(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| ((i * 37 % 101) as f32 - 50.0) / 23.0)
            .collect()
    }

    #[test]
    fn roundtrip_error_is_small() {
        let x = data(256);
        for (dt, bytes, tol) in [
            (DType::Q8_0, quantize_q8_0(&x), 0.02),
            (DType::Q4_0, quantize_q4_0(&x), 0.3), // ggml q4_0 clamps one end
        ] {
            let y = decode_f32(dt, &bytes);
            let err = x
                .iter()
                .zip(&y)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(err < tol, "{dt:?}: max err {err}");
        }
        assert!(quantize_q8_0(&[0.0; 32])[2..].iter().all(|&b| b == 0));
    }

    #[test]
    fn quantized_matmul_matches_dequantized() {
        let (rows_, n) = (4096, 256); // large enough to take the threaded path
        let w = data(rows_ * n);
        let x: Vec<f32> = data(n + 7)[7..].to_vec();
        let mut xq = QVec::default();
        // Scale for errors: sum_j |w_ij x_j| (same for every row up to permutation).
        let mag: f32 = w[..n].iter().zip(&x).map(|(a, b)| (a * b).abs()).sum();
        for (bytes, wt, dt) in [
            (
                quantize_q8_0(&w),
                Weights::from_q8_0(&quantize_q8_0(&w)),
                DType::Q8_0,
            ),
            (
                quantize_q4_0(&w),
                Weights::from_q4_0(&quantize_q4_0(&w)),
                DType::Q4_0,
            ),
        ] {
            let deq = Weights::F32(decode_f32(dt, &bytes));
            let mut exact = vec![0.0; rows_];
            matmul(&mut exact, &x, &deq, &mut xq, 1);
            for th in [1, 3] {
                let mut got = vec![0.0; rows_];
                matmul(&mut got, &x, &wt, &mut xq, th);
                // Only the q8 activation rounding separates `got` from `exact`.
                for (g, e) in got.iter().zip(&exact) {
                    assert!((g - e).abs() < 0.01 * mag, "{dt:?} {th}: {g} vs {e}");
                }
                let mut row = vec![0.0; n];
                wt.row(3, n, &mut row);
                assert_eq!(row, decode_f32(dt, &bytes)[3 * n..4 * n]);
            }
        }
    }
}
