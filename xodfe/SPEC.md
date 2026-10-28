# XODFe file format, version 1

XODFe is a single-file container for a model. One file holds the tensors, typed
metadata (architecture, hyperparameters) and the tokenizer. It replaces
safetensors for the Limits project. The header is binary rather than JSON, and a
reader validates every byte of it before using any of it.

All integers are **little-endian**. Offsets are in bytes.

## 1. File layout

| Offset | Size         | Field                                                |
|-------:|--------------|------------------------------------------------------|
| 0      | 8            | magic: `58 4F 44 46 65 00 00 00` (`"XODFe\0\0\0"`)    |
| 8      | 4            | `version` u32, must be `1`                           |
| 12     | 4            | `flags` u32, must be `0` (every bit is reserved)     |
| 16     | 8            | `header_len` u64: byte length of the header, CRC included |
| 24     | `header_len` | header (§2)                                          |
| 24+header_len | pad   | zero bytes up to `data_start`                        |
| `data_start`  | …     | data section (§4)                                    |

`data_start = align_up(24 + header_len, 64)`, where `align_up(x, a) = ceil(x / a) * a`.
The bytes from `24 + header_len` up to `data_start` **must** be zero.

## 2. Header

```
u32            n_meta
n_meta  x MetaEntry
u32            n_tensors
n_tensors x TensorEntry
u32            header_crc     CRC-32 of every header byte before this field
```

`header_len >= 12`. The entries must use up exactly `header_len - 4` bytes.
Leftover bytes are an error.

**str** = `u32 len` followed by `len` bytes of valid UTF-8 (no terminator).

### 2.1 MetaEntry

```
str   key            unique among all metadata keys
u8    tag
...   value          encoding depends on tag
```

| tag | type    | value encoding                                  |
|----:|---------|-------------------------------------------------|
| 0   | u64     | u64                                             |
| 1   | i64     | i64 (two's complement)                          |
| 2   | f64     | IEEE-754 binary64                               |
| 3   | bool    | u8, `0` or `1` only                             |
| 4   | str     | str                                             |
| 5   | bytes   | u64 n, then n bytes                             |
| 6   | [f32]   | u64 n, then n x IEEE-754 binary32               |
| 7   | [str]   | u64 n, then n x str                             |

### 2.2 TensorEntry

```
str      name        unique among all tensor names
u8       dtype       §3
u8       rank        0..=8  (rank 0 = scalar, 1 element)
u64[rank] dims       outermost first, row-major (C order)
u64      offset      relative to data_start, multiple of 64
u64      nbytes      must equal byte_len(dtype, dims)
u32      crc         CRC-32 of the tensor's nbytes data bytes
```

## 3. dtypes

`numel` is the product of `dims`, computed with overflow checks. For rank 0 it is 1.

| tag | name  | byte_len                     | element / block encoding |
|----:|-------|------------------------------|--------------------------|
| 0   | f32   | 4·numel                      | IEEE binary32 |
| 1   | f16   | 2·numel                      | IEEE binary16 |
| 2   | bf16  | 2·numel                      | bfloat16, the top 16 bits of a binary32 |
| 3   | i8    | numel                        | two's complement |
| 4   | q8_0  | (numel/32)·34                | block = f16 scale `d`, then 32 x i8 `q`; value = `q·d` |
| 5   | q4_0  | (numel/32)·18                | block = f16 scale `d`, then 16 bytes. Byte `j` holds element `j` in its low nibble and element `j+16` in its high nibble; value = `(nibble − 8)·d` |

The quantized dtypes (q8_0, q4_0) require `rank >= 1` and a last dimension
divisible by 32. Each block covers 32 consecutive elements of one row.
Overflow in `numel` or `byte_len` is an error.

## 4. Data section

- Every tensor occupies `[data_start + offset, data_start + offset + nbytes)`.
- `offset % 64 == 0`. `data_start` is also 64-aligned, so a memory-mapped tensor
  lands on a 64-byte (cache line / AVX-512) boundary.
- Non-empty tensors must not overlap. Empty tensors (`nbytes == 0`) are allowed
  and take up no space. Writers put them at offset 0.
- The file ends exactly at the largest `offset + nbytes`. If there are no
  non-empty tensors, the file ends at `data_start`. Any trailing byte is an error.
- Writers fill the gaps between tensors with zeros. Readers do not check the gaps.
- The reference writer stores tensors in insertion order, each at the next
  64-aligned offset.

## 5. Checksums

CRC-32 here is CRC-32/ISO-HDLC, the same CRC as zlib's `crc32`: polynomial
`0xEDB88320` reflected, init `0xFFFFFFFF`, final xor `0xFFFFFFFF`. For example,
`crc32("123456789") = 0xCBF43926`.

- The header CRC is **always** checked before the header is parsed.
- Tensor CRCs are checked on demand (`Model::verify_checksums`), or at load time
  with `Model::open(path, true)`. `xodfe verify` always checks them.

## 6. Validation (normative)

A conforming reader rejects the file with a typed error, and never panics,
reads out of bounds or allocates without bound, in each of these cases:

1. Magic is wrong (`BadMagic`), version is not 1 (`UnsupportedVersion`), or flags are not 0 (`UnknownFlags`).
2. `header_len < 12`, or `24 + header_len` overflows or exceeds the file size (`BadHeaderLen`).
3. The header CRC does not match (`HeaderCrc`).
4. Any read runs past the end of the header (`Truncated`). An array count n is
   rejected before allocation if `n x min_element_size` exceeds the bytes that remain.
5. A string is not valid UTF-8 (`BadUtf8`), a value tag is unknown (`BadValueTag`), a bool byte is not 0 or 1 (`BadBool`), or a key appears twice (`DuplicateKey`).
6. A dtype is unknown (`BadDType`), the rank is above 8 (`RankTooLarge`), dims or byte length overflow (`DimOverflow`), a quantized tensor has the wrong shape (`BadBlockShape`), or `nbytes` does not equal `byte_len` (`SizeMismatch`).
7. An offset is not 64-aligned (`Misaligned`), or `offset + nbytes` overflows or runs past the end of the file (`OutOfBounds`).
8. Two tensors overlap (`Overlap`) or share a name (`DuplicateTensor`).
9. The header has leftover bytes (`TrailingHeaderBytes`), the padding is non-zero (`NonZeroPadding`), or the file has bytes after the last tensor (`TrailingData`).
10. A tensor checksum does not match, when checksums are verified (`TensorCrc`).

Loading never executes code. Nothing in the file is interpreted except the
fields above.

## 7. Conventional metadata keys

LLaMA-family models (written by `xodfe convert` from a llama2.c checkpoint):

| key | type | meaning |
|-----|------|---------|
| `general.architecture` | str | `"llama"` |
| `general.source` | str | where the weights came from |
| `llama.dim`, `llama.hidden_dim`, `llama.n_layers`, `llama.n_heads`, `llama.n_kv_heads`, `llama.vocab_size`, `llama.seq_len` | u64 | hyperparameters |
| `llama.shared_classifier` | bool | if true, there is no `output.weight` and the output layer reuses `tok_embeddings.weight` |
| `llama.rope_theta` | f64 | 10000 |
| `llama.norm_eps` | f64 | 1e-5 |
| `tokenizer.model` | str | `"llama2c-bpe"` (score-greedy BPE, as in llama2.c) |
| `tokenizer.tokens` | [str] | vocabulary, indexed by token id |
| `tokenizer.scores` | [f32] | merge scores, same length as the vocabulary |
| `tokenizer.max_token_length` | u64 | longest token in bytes |
| `tokenizer.bos_id`, `tokenizer.eos_id` | u64 | 1 and 2 |

Tensor names: `tok_embeddings.weight [vocab, dim]`, `norm.weight [dim]`,
`output.weight [vocab, dim]` (only when the classifier is not shared), and for
each layer `i` the tensors `layers.{i}.attention_norm.weight [dim]`,
`layers.{i}.attention.{wq,wo}.weight [dim, dim]`,
`layers.{i}.attention.{wk,wv}.weight [kv_dim, dim]`,
`layers.{i}.ffn_norm.weight [dim]`,
`layers.{i}.feed_forward.{w1,w3}.weight [hidden, dim]` and
`layers.{i}.feed_forward.w2.weight [dim, hidden]`. All matrices are `[out, in]`.
The RoPE frequency tables in llama2.c files are not stored; they are recomputed
from `rope_theta`.

Imported safetensors metadata (`__metadata__`, string to string) is stored as
str entries named `safetensors.<key>`. Exporting to safetensors writes only these
entries back. Other metadata, such as the tokenizer, has no safetensors
equivalent and is dropped.

## 8. Interop

- **safetensors to XODFe**: dtypes F32, F16, BF16 and I8 are supported. Any other
  dtype is rejected with a clear error. Tensors are written in order of their
  safetensors data offsets.
- **XODFe to safetensors**: only f32, f16 and bf16 tensors can be exported. The
  JSON header is padded with spaces to a multiple of 8 bytes, and tensor bytes
  are copied unchanged.
- **llama2.c `.bin` (+ `tokenizer.bin`) to XODFe**: the weights are copied bit for
  bit. Every token in `tokenizer.bin` must be valid UTF-8 (true of the llama2.c
  vocabulary).

## Inference (`xodfe run`, `xodfe bench`)

`run` reads only the `.xodfe` file. Required metadata: `general.architecture = "llama"`,
`llama.{dim,hidden_dim,n_layers,n_heads,n_kv_heads,vocab_size,seq_len}` (u64); optional
`llama.rope_theta` / `llama.norm_eps` (f64, defaults 10000 / 1e-5); `tokenizer.tokens` ([str]),
`tokenizer.scores` ([f32]), `tokenizer.bos_id` / `eos_id` (u64, defaults 1 / 2).
Tensors use llama2.c names (`tok_embeddings.weight`, `layers.N.attention.wq.weight`, ...,
`norm.weight`, optional `output.weight`; absent = classifier shared with the embeddings), any
dtype. Norm vectors and f32/f16/bf16 matrices are converted to f32 at load. q8_0/q4_0
matrices stay quantized in memory (codes plus per-block scales) and are never expanded to f32:
each matmul quantizes the activation vector to q8_0 blocks once, then computes per-block
integer dot products scaled by `d_w * d_x` (as in llama2.c `runq.c`). Matmuls with at least
2^20 weights are split by rows across `--threads` scoped threads (default: available
parallelism); smaller ones run on one thread because spawning costs more than the work. The model is Llama-2: RMSNorm, RoPE, multi-head attention
with a KV cache (GQA when `n_kv_heads < n_heads`), SwiGLU FFN. Tokenization, decoding and the
sampler (greedy / temperature / top-p, xorshift RNG seeded by `--seed`; 0 = time-based) follow
llama2.c `run.c`. `-n` counts positions including the prompt and is capped at `seq_len`.

## Quantization (`xodfe quantize IN OUT --to q8_0|q4_0`)

Every rank-2 f32/f16/bf16 tensor whose last dimension is a multiple of 32 is quantized
(for Llama: all projection matrices and the embedding table); everything else (norms,
other dtypes) is copied unchanged. All metadata is copied and
`general.quantization = "<dtype>"` is added. q8_0: `d = f16(amax/127)`, `q = round(x/d)`.
q4_0 (ggml convention): `m` = the value with the largest magnitude, `d = f16(m/-8)`,
nibble `= clamp(floor(x/d + 8.5), 0, 15)`. The output is re-validated with all checksums.
