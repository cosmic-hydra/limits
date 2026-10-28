# xodfe

XODFe is a safetensors alternative for Limits. One validated, memory-mappable
file holds the tensors, typed metadata and the tokenizer. The crate has no
dependencies. The byte-level format is in [SPEC.md](SPEC.md).

```
cargo build --release
B=target/release/xodfe
$B convert models/stories15M.bin models/stories15M.xodfe --tokenizer models/tokenizer.bin
$B info    models/stories15M.xodfe
$B verify  models/stories15M.xodfe                 # structure + every CRC
$B convert model.safetensors model.xodfe
$B export  model.xodfe roundtrip.safetensors
python3 -I tools/compare_safetensors.py model.safetensors roundtrip.safetensors
python3 -I tools/check_llama2c.py models/stories15M.bin stories15M.export.safetensors

$B quantize models/stories15M.xodfe models/stories15M.q8_0.xodfe --to q8_0
$B run     models/stories15M.q8_0.xodfe --prompt "Once upon a time" --temp 0 --threads 4
$B bench   models/stories15M.q4_0.xodfe --ref models/stories15M.xodfe
```

Measured numbers are in [BENCH.md](BENCH.md).

`models/` holds downloaded weights and `target/` holds build output. Both are
git-ignored.
