# XODFe benchmarks

Every number below was measured on 2026-10-28 with `xodfe bench`, on a cloud VM with an
Intel Xeon @ 2.80GHz, 4 vCPUs (AVX2/AVX-512 available, but the binary is built for the
default x86-64 target, so SSE2 only). The build was `cargo build --release` (LTO,
codegen-units=1). The machine is shared and noisy: tokens/sec moved by up to ±20%
between runs, so each one is given as the range over 3 runs.

Model: karpathy's stories15M (Llama-2 architecture, dim 288, 6 layers, vocab 32000).

Reproduce with:

```
B=target/release/xodfe
$B convert  models/stories15M.bin models/stories15M.xodfe --tokenizer models/tokenizer.bin
$B quantize models/stories15M.xodfe models/stories15M.q8_0.xodfe --to q8_0
$B quantize models/stories15M.xodfe models/stories15M.q4_0.xodfe --to q4_0
$B bench models/stories15M.q8_0.xodfe --threads 4 --ref models/stories15M.xodfe
$B bench models/stories15M.export.safetensors       # JSON header parse time
```

## File sizes

| file | bytes | contents |
|---|---:|---|
| `stories15M.bin` (llama2.c checkpoint) | 60,816,028 | weights + 28-byte config; tokenizer is a separate file (433,869 bytes) |
| `stories15M.export.safetensors` | 60,772,424 | weights only |
| `stories15M.xodfe` (f32) | 61,205,184 | weights, config, tokenizer (438,336-byte header) |
| `stories15M.q8_0.xodfe` | 16,590,528 | 27% of f32 |
| `stories15M.q4_0.xodfe` | 8,996,544 | 15% of f32 |

The `quantize` command quantizes 43 tensors: every projection matrix plus the embedding
table, which this model also uses as the classifier. Norm vectors stay f32.

## Load time (open + full structural validation, median of 21)

| file | mmap (`Model::open`) | read into Vec (`fs::read` + `from_vec`) |
|---|---:|---:|
| f32 | 2.1 ms | 47.8 ms |
| q8_0 | 2.7 ms | 5.6 ms |
| q4_0 | 2.9 ms | 3.6 ms |

With mmap, opening the file only parses and validates the header. Tensor pages load
later, when they are first touched. Checking every tensor CRC took 38.8 ms for f32,
11.5 ms for q8_0 and 5.6 ms for q4_0 (about 1.5 GB/s). `Transformer::load` took
43 to 58 ms for f32 (it copies the weights to f32 Vecs) and 9 to 25 ms for the
quantized files.

## Generation speed (greedy, prompt "Once upon a time", up to 256 positions)

tok/s follows run.c's convention: steps after the first, divided by the time after the first.

| weights | 1 thread | 4 threads |
|---|---:|---:|
| f32 | 93 – 105 | 110 – 149 |
| q8_0 | 222 – 229 | 284 – 310 |
| q4_0 | 105 – 129 | 139 – 170 |

- q8_0 is the fastest. It reads 3.7x fewer bytes per token than f32, and its inner loop
  is an i8 x i8 to i32 block dot that LLVM vectorizes (`pmaddwd`).
- q4_0 is smaller than q8_0 but slower. On every row it first unpacks nibbles to i8
  (into a row-sized i8 scratch buffer, not an f32 copy), and with SSE2 that unpacking
  costs more than the bandwidth it saves.
- Threads help only a little. A scoped thread spawn costs tens of microseconds, so only
  matmuls with at least 2^20 weights are split. In stories15M that is only the
  32000x288 classifier. Lowering the cutoff to 2^17 also parallelized the FFN matrices,
  and 4 threads then measured slower than 1 (f32 63 vs 90 tok/s). A persistent worker
  pool would be needed to scale on a model this small.
- **llama2.c run.c: not measured.** Compiling the downloaded run.c was blocked by this
  session's permission policy, so there is no run.c speed number here. Greedy
  correctness is still checked against run.c semantics through `tools/ref_llama2c.py`
  (an independent numpy port) in `tests/parity.rs`. On 3 prompts x 200 steps the f32
  path produces identical text. To compare speed yourself:
  `gcc -Ofast -o run run.c -lm && ./run models/stories15M.bin -z models/tokenizer.bin -t 0 -n 256 -i "Once upon a time"`.
  Set `XODFE_LLAMA2C_RUN=./run` to run the parity test against the real binary.

## Output agreement with f32 (greedy, `bench --ref`)

The number of leading generated tokens (after the prompt, 256 positions total) that are
identical to the f32 model's:

| prompt | q8_0 | q4_0 |
|---|---:|---:|
| "Once upon a time" | 94 / 251 | 17 / 251 |
| "The little dog" | 21 / 252 | 1 / 252 |
| "Lily wanted to" | 9 / 251 | 0 / 251 |

Greedy decoding on a 15M-parameter model is brittle. Once one near-tied argmax flips, the
two continuations diverge for good. Both quantized models still write fluent stories
(see `xodfe run ... --temp 0`), but not the same stories. There are two sources of error:
the weights (q8_0 or q4_0) and the q8_0 rounding of the activations inside every
quantized matmul. The embedding table is quantized too.

## XODFe vs safetensors

### Header parse time (median of 21, parse only, no tensor data touched)

| file | header bytes | parse |
|---|---:|---:|
| small public model, safetensors (`models/st/model.safetensors`) | JSON | 0.043 ms |
| same tensors converted to XODFe (`models/st/model.xodfe`) | 4,352 | 0.024 ms (incl. full validation) |
| stories15M safetensors export | JSON | 0.046 ms |
| stories15M XODFe f32 | 438,336 | 1.8 – 2.8 ms (incl. 32,000-entry tokenizer and validation) |

For the same tensors, the binary header parses about 1.8x faster than our JSON parser
reads the safetensors header, and that parse time already includes all validation. The
stories15M XODFe header is about 100x larger only because it also holds the
tokenizer. safetensors cannot store a tokenizer at all.

### What each format rejects

The XODFe column is enforced by `format::parse` and covered by `tests/format.rs`. The
safetensors column describes the format specification and the reference `safetensors`
library as documented. It was not tested here, because that library is not installed.

| check | XODFe | safetensors |
|---|---|---|
| magic + version | yes | no magic, no version |
| header length vs file size | yes | yes |
| header integrity checksum | yes (crc32) | no |
| string lengths / UTF-8 | yes | JSON must be valid UTF-8 |
| rank limit | yes (<= 8) | no explicit limit |
| dims product overflow | yes (checked) | yes (reference impl) |
| byte length == dims x dtype (block-aware for q8_0/q4_0) | yes | yes (no block dtypes exist) |
| offsets in bounds | yes | yes |
| overlapping tensors | yes | yes (reference impl requires contiguous, gap-free data) |
| 64-byte alignment | yes | no (8-byte alignment of the header is only recommended) |
| duplicate names | yes | JSON duplicate keys: parser-dependent |
| per-tensor data checksum | yes (crc32, at load with a flag or `verify`) | no |
| typed metadata (u64/f64/[str]/...) | yes | string-to-string only |
| tokenizer + config in the same file | yes | no |
