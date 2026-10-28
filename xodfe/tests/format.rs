//! Round-trip, malformed-input and fuzz tests for the XODFe reader/writer.

use xodfe::format::{encode_raw, parse, RawTensor, ALIGN, HEADER_START};
use xodfe::num::{bf16_to_f32, crc32, f16_to_f32, f32_to_bf16, f32_to_f16};
use xodfe::{safetensors, DType, Error, Model, Value, Writer};

fn sample_writer() -> Writer {
    let mut w = Writer::new();
    w.meta("general.architecture", Value::Str("llama".into()))
        .meta("u", Value::U64(u64::MAX))
        .meta("i", Value::I64(-7))
        .meta("f", Value::F64(0.125))
        .meta("b", Value::Bool(true))
        .meta("bytes", Value::Bytes(vec![0, 255, 7]))
        .meta("scores", Value::F32s(vec![1.5, -2.0]))
        .meta(
            "toks",
            Value::Strs(vec!["<s>".into(), "héllo".into(), String::new()]),
        );
    let n = 64u64;
    let pat = |sz: usize| (0..sz).map(|i| (i * 37 + 11) as u8).collect::<Vec<u8>>();
    w.tensor_f32("a.f32", &[2, 3], &[1.0, -2.0, 3.5, 0.0, 1e-8, 6.0])
        .unwrap();
    w.tensor("b.f16", DType::F16, &[n], pat(2 * n as usize))
        .unwrap();
    w.tensor("c.bf16", DType::BF16, &[4, 4], pat(32)).unwrap();
    w.tensor("d.i8", DType::I8, &[5], pat(5)).unwrap();
    w.tensor("e.q8", DType::Q8_0, &[2, 32], pat(68)).unwrap();
    w.tensor("f.q4", DType::Q4_0, &[64], pat(36)).unwrap();
    w.tensor("g.scalar", DType::F32, &[], pat(4)).unwrap();
    w.tensor("h.empty", DType::F32, &[0, 3], vec![]).unwrap();
    w
}

fn sample() -> Vec<u8> {
    sample_writer().to_bytes()
}

fn header_len(b: &[u8]) -> usize {
    u64::from_le_bytes(b[16..24].try_into().unwrap()) as usize
}

/// Recompute the header CRC after a mutation so structural checks are reached.
fn fix_crc(b: &mut [u8]) {
    if b.len() < 24 {
        return;
    }
    let hl = header_len(b);
    if hl < 4 || hl > b.len() - 24 {
        return;
    }
    let end = 24 + hl;
    let c = crc32(&b[24..end - 4]);
    b[end - 4..end].copy_from_slice(&c.to_le_bytes());
}

fn raw(name: &str, dtype: DType, dims: &[u64], offset: u64, data: &[u8]) -> RawTensor {
    RawTensor {
        name: name.into(),
        dtype: dtype as u8,
        dims: dims.into(),
        offset,
        nbytes: data.len() as u64,
        crc: crc32(data),
    }
}

#[test]
fn crc32_known_vectors() {
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    let long: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
    // slicing-by-8 must agree with the bytewise definition
    let mut c = !0u32;
    for &b in &long {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    assert_eq!(crc32(&long), !c);
}

#[test]
fn f16_bf16_conversions() {
    for h in 0..=u16::MAX {
        let f = f16_to_f32(h);
        if f.is_nan() {
            assert!(f16_to_f32(f32_to_f16(f)).is_nan());
        } else {
            assert_eq!(f32_to_f16(f), h, "f16 {h:#06x}");
        }
        let g = bf16_to_f32(h);
        if !g.is_nan() {
            assert_eq!(f32_to_bf16(g), h);
        }
    }
    assert_eq!(f32_to_f16(65520.0), 0x7c00); // rounds to inf
    assert_eq!(f32_to_f16(1.0 + 1.0 / 2048.0), 0x3c00); // tie -> even
    assert_eq!(f32_to_f16(1e-10), 0);
    assert_eq!(f32_to_bf16(1.0 + 1.0 / 256.0), 0x3f80);
}

#[test]
fn roundtrip_every_dtype() {
    let w = sample_writer();
    let bytes = w.to_bytes();
    let m = Model::from_vec(bytes.clone(), true).unwrap();
    assert_eq!(m.header.meta.len(), 8);
    assert_eq!(
        m.meta("toks"),
        Some(&Value::Strs(vec![
            "<s>".into(),
            "héllo".into(),
            String::new()
        ]))
    );
    assert_eq!(m.meta("u"), Some(&Value::U64(u64::MAX)));
    let names: Vec<_> = m.tensors().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        ["a.f32", "b.f16", "c.bf16", "d.i8", "e.q8", "f.q4", "g.scalar", "h.empty"]
    );
    for t in m.tensors() {
        assert_eq!(
            (m.header.data_start + t.offset) % ALIGN,
            0,
            "{} aligned",
            t.name
        );
        assert_eq!(m.to_f32(t).len() as u64, t.numel());
    }
    assert_eq!(
        m.to_f32(m.tensor("a.f32").unwrap()),
        [1.0, -2.0, 3.5, 0.0, 1e-8, 6.0]
    );
    // Re-encoding the parsed model reproduces the file byte for byte.
    let mut w2 = Writer::new();
    for (k, v) in &m.header.meta {
        w2.meta(k, v.clone());
    }
    for t in m.tensors() {
        w2.tensor(&t.name, t.dtype, &t.dims, m.data(t).to_vec())
            .unwrap();
    }
    assert_eq!(w2.to_bytes(), bytes);
}

#[test]
fn file_roundtrip_mmap_and_safetensors() {
    let dir = std::env::temp_dir().join(format!("xodfe-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("m.xodfe");
    let mut w = Writer::new();
    w.meta("safetensors.format", Value::Str("pt".into()));
    w.tensor_f32("x", &[3], &[1.0, 2.0, 3.0]).unwrap();
    w.tensor("y", DType::F16, &[2], vec![0, 0x3c, 0, 0xc0])
        .unwrap();
    w.tensor("z", DType::BF16, &[1], vec![0x80, 0x3f]).unwrap();
    w.write(&p).unwrap();
    let m = Model::open(&p, true).unwrap();
    assert_eq!(m.to_f32(m.tensor("y").unwrap()), [1.0, -2.0]);
    let st = safetensors::export(&m).unwrap();
    let back = Model::from_vec(safetensors::import(&st).unwrap().to_bytes(), true).unwrap();
    assert_eq!(back.header.meta, m.header.meta);
    for t in m.tensors() {
        let u = back.tensor(&t.name).unwrap();
        assert_eq!(
            (u.dtype, &u.dims, back.data(u)),
            (t.dtype, &t.dims, m.data(t))
        );
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn quantized_dtypes_cannot_export_to_safetensors() {
    let m = Model::from_vec(sample(), true).unwrap();
    assert!(matches!(safetensors::export(&m), Err(Error::Format(_))));
}

#[test]
fn truncated_at_every_length() {
    let b = sample();
    for n in 0..b.len() {
        let r = parse(&b[..n]);
        assert!(r.is_err(), "prefix of {n} bytes accepted");
    }
    assert!(parse(&b).is_ok());
}

#[test]
fn preamble_errors() {
    let b = sample();
    let mut x = b.clone();
    x[0] = b'Y';
    assert_eq!(parse(&x).unwrap_err(), Error::BadMagic);
    let mut x = b.clone();
    x[8] = 2;
    assert_eq!(parse(&x).unwrap_err(), Error::UnsupportedVersion(2));
    let mut x = b.clone();
    x[12] = 1;
    assert_eq!(parse(&x).unwrap_err(), Error::UnknownFlags(1));
    for bad in [0u64, 11, u64::MAX, u64::MAX - 10, b.len() as u64] {
        let mut x = b.clone();
        x[16..24].copy_from_slice(&bad.to_le_bytes());
        assert_eq!(parse(&x).unwrap_err(), Error::BadHeaderLen(bad));
    }
}

#[test]
fn every_header_byte_flip_is_rejected_by_crc() {
    let b = sample();
    let end = 24 + header_len(&b);
    for i in HEADER_START as usize..end {
        let mut x = b.clone();
        x[i] ^= 0x41;
        assert!(
            matches!(parse(&x), Err(Error::HeaderCrc { .. })),
            "byte {i}"
        );
    }
}

#[test]
fn every_header_byte_corruption_with_fixed_crc_is_typed() {
    // With the CRC recomputed, the structural checks must catch (or tolerate)
    // every single-byte change without panicking.
    let b = sample();
    let end = 24 + header_len(&b) - 4;
    for i in 24..end {
        for v in [0x00, 0x01, 0x7f, 0x80, 0xff] {
            let mut x = b.clone();
            x[i] = v;
            fix_crc(&mut x);
            let _ = Model::from_vec(x, true);
        }
    }
}

#[test]
fn padding_and_data_section_errors() {
    let b = sample();
    let hend = 24 + header_len(&b);
    let ds = (hend as u64).div_ceil(ALIGN) * ALIGN;
    if (ds as usize) > hend {
        let mut x = b.clone();
        x[hend] = 1;
        assert_eq!(parse(&x).unwrap_err(), Error::NonZeroPadding);
    }
    let mut x = b.clone();
    x.extend_from_slice(&[0; 3]);
    assert_eq!(parse(&x).unwrap_err(), Error::TrailingData(3));
    // Flip a data byte: header still parses, checksum verification fails.
    let mut x = b.clone();
    let last = x.len() - 1;
    x[last] ^= 1;
    assert!(parse(&x).is_ok());
    assert!(matches!(
        Model::from_vec(x, true),
        Err(Error::TensorCrc { .. })
    ));
}

#[test]
fn structural_errors_are_typed() {
    let d64 = vec![0u8; 64];
    let mk = |ts: Vec<RawTensor>, data: &[u8]| parse(&encode_raw(0, &[], &ts, data));
    let t = |n: &str, off: u64| raw(n, DType::F32, &[16], off, &d64);

    assert!(mk(vec![t("a", 0)], &d64).is_ok());
    let two = vec![0u8; 128];
    assert_eq!(
        mk(vec![t("a", 0), t("b", 0)], &two).unwrap_err(),
        Error::Overlap {
            a: "a".into(),
            b: "b".into()
        }
    );
    let big = raw("a", DType::F32, &[24], 0, &[0; 96]);
    assert!(matches!(
        mk(vec![big, t("b", 64)], &[0; 128]),
        Err(Error::Overlap { .. })
    ));
    assert_eq!(
        mk(vec![t("a", 0), t("a", 64)], &two).unwrap_err(),
        Error::DuplicateTensor("a".into())
    );
    assert_eq!(
        mk(vec![t("a", 32)], &two).unwrap_err(),
        Error::Misaligned {
            name: "a".into(),
            offset: 32
        }
    );
    assert_eq!(
        mk(vec![t("a", 64)], &d64).unwrap_err(),
        Error::OutOfBounds { name: "a".into() }
    );
    let far = t("a", u64::MAX - 63);
    assert_eq!(
        mk(vec![far], &d64).unwrap_err(),
        Error::OutOfBounds { name: "a".into() }
    );

    let mut sz = t("a", 0);
    sz.nbytes = 60;
    assert_eq!(
        mk(vec![sz], &d64).unwrap_err(),
        Error::SizeMismatch {
            name: "a".into(),
            expected: 64,
            got: 60
        }
    );
    let mut huge = t("a", 0);
    huge.dims = vec![u64::MAX, 2];
    assert_eq!(
        mk(vec![huge], &d64).unwrap_err(),
        Error::DimOverflow("a".into())
    );
    let mut huge = t("a", 0);
    huge.dims = vec![u64::MAX / 2];
    assert_eq!(
        mk(vec![huge], &d64).unwrap_err(),
        Error::DimOverflow("a".into())
    );
    let mut rank = t("a", 0);
    rank.dims = vec![1; 9];
    assert_eq!(
        mk(vec![rank], &d64).unwrap_err(),
        Error::RankTooLarge {
            name: "a".into(),
            rank: 9
        }
    );
    let mut dt = t("a", 0);
    dt.dtype = 6;
    assert_eq!(mk(vec![dt], &d64).unwrap_err(), Error::BadDType(6));
    let q = raw("q", DType::Q8_0, &[2, 16], 0, &[0; 34]);
    assert_eq!(
        mk(vec![q], &d64[..34]).unwrap_err(),
        Error::BadBlockShape("q".into())
    );

    let dup = [
        ("k".to_string(), Value::U64(1)),
        ("k".to_string(), Value::Bool(false)),
    ];
    assert_eq!(
        parse(&encode_raw(0, &dup, &[], &[])).unwrap_err(),
        Error::DuplicateKey("k".into())
    );
}

#[test]
fn bad_utf8_bool_tag_and_lengths() {
    let base = encode_raw(0, &[("key".into(), Value::Str("ab".into()))], &[], &[]);
    let at = |x: &[u8], pat: &[u8]| x.windows(pat.len()).position(|w| w == pat).unwrap();

    let mut x = base.clone();
    let i = at(&x, b"key");
    x[i + 1] = 0xff;
    fix_crc(&mut x);
    assert!(matches!(parse(&x), Err(Error::BadUtf8 { .. })));

    let mut x = base.clone();
    let i = at(&x, b"key") + 3;
    x[i] = 9; // value tag
    fix_crc(&mut x);
    assert_eq!(parse(&x).unwrap_err(), Error::BadValueTag(9));

    let mut x = base.clone();
    x[i + 1..i + 5].copy_from_slice(&u32::MAX.to_le_bytes()); // string length
    fix_crc(&mut x);
    assert!(matches!(parse(&x), Err(Error::Truncated { .. })));

    let mut x = encode_raw(0, &[("b".into(), Value::Bool(true))], &[], &[]);
    let i = at(&x, b"b") + 2;
    x[i] = 2;
    fix_crc(&mut x);
    assert_eq!(parse(&x).unwrap_err(), Error::BadBool(2));

    // [f32] / [str] counts that cannot fit must not allocate.
    for v in [
        Value::F32s(vec![1.0]),
        Value::Strs(vec!["s".into()]),
        Value::Bytes(vec![1]),
    ] {
        let mut x = encode_raw(0, &[("a".into(), v)], &[], &[]);
        let i = at(&x, b"a") + 2;
        x[i..i + 8].copy_from_slice(&(u64::MAX / 2).to_le_bytes());
        fix_crc(&mut x);
        assert!(matches!(parse(&x), Err(Error::Truncated { .. })));
    }

    // Extra bytes between the last tensor entry and the header CRC.
    let hend = 24 + header_len(&base);
    let mut x = base[..hend - 4].to_vec();
    x.extend_from_slice(&[0; 8]); // 4 junk bytes + CRC slot
    let hl = (header_len(&base) + 4) as u64;
    x[16..24].copy_from_slice(&hl.to_le_bytes());
    x.resize(
        (x.len() as u64).div_ceil(ALIGN) as usize * ALIGN as usize,
        0,
    );
    fix_crc(&mut x);
    assert_eq!(parse(&x).unwrap_err(), Error::TrailingHeaderBytes(4));
}

#[test]
fn fuzz_never_panics() {
    let base = sample();
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut rng = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let (mut ok, mut err) = (0, 0);
    for it in 0..25_000 {
        let mut x = base.clone();
        for _ in 0..1 + rng() % 4 {
            let r = rng();
            let i = (r >> 8) as usize % x.len();
            match r % 4 {
                0 => x[i] = r as u8 ^ (r >> 32) as u8,
                1 => x[i] ^= 1 << (r % 8),
                2 => x.truncate(i),
                _ => x.insert(i, (r >> 16) as u8),
            }
            if x.is_empty() {
                x.push(0);
            }
        }
        if it % 2 == 0 {
            fix_crc(&mut x); // half the time get past the header CRC
        }
        match Model::from_vec(x, true) {
            Ok(m) => {
                for t in m.tensors() {
                    let _ = m.to_f32(t);
                }
                ok += 1
            }
            Err(e) => {
                let _ = e.to_string();
                err += 1
            }
        }
    }
    assert!(err > 15_000 && ok + err == 25_000, "ok={ok} err={err}");
}

#[test]
fn fuzz_safetensors_and_json_never_panic() {
    let mut w = Writer::new();
    w.meta("safetensors.k", Value::Str("v\"\n".into()));
    w.tensor_f32("x", &[2], &[1.0, 2.0]).unwrap();
    let st = safetensors::export(&Model::from_vec(w.to_bytes(), true).unwrap()).unwrap();
    let mut s: u64 = 12345;
    for _ in 0..20_000 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let mut x = st.clone();
        let i = (s >> 8) as usize % x.len();
        x[i] = (s >> 40) as u8;
        if s.is_multiple_of(3) {
            x.truncate(i);
        }
        let _ = safetensors::import(&x);
        let _ = xodfe::llama2c::import(&x, Some(&x));
    }
}

fn st_file(header: &str, data_len: usize) -> Vec<u8> {
    let mut b = (header.len() as u64).to_le_bytes().to_vec();
    b.extend_from_slice(header.as_bytes());
    b.extend(std::iter::repeat_n(0u8, data_len));
    b
}

#[test]
fn safetensors_rejects_overlap_gaps_and_trailing_bytes() {
    let t = |n: &str, b: u32, e: u32| {
        format!("\"{n}\":{{\"dtype\":\"F32\",\"shape\":[1],\"data_offsets\":[{b},{e}]}}")
    };
    let overlap = format!("{{{},{}}}", t("a", 0, 4), t("b", 0, 4));
    assert!(safetensors::import(&st_file(&overlap, 4)).is_err());
    let trailing = format!("{{{}}}", t("a", 0, 4));
    assert!(safetensors::import(&st_file(&trailing, 8)).is_err());
    let gap = format!("{{{},{}}}", t("a", 0, 4), t("b", 8, 12));
    assert!(safetensors::import(&st_file(&gap, 12)).is_err());
    let ok = format!("{{{},{}}}", t("b", 4, 8), t("a", 0, 4));
    assert!(safetensors::import(&st_file(&ok, 8)).is_ok());
}

/// A tiny llama whose metadata claims a 2^24 context: run state must be
/// sized by the requested positions, not by seq_len.
#[test]
fn run_state_is_sized_by_requested_steps() {
    use xodfe::llama::{generate, Sampler, Tokenizer, Transformer};
    let (d, h, v, seq) = (64u64, 32u64, 4u64, 1u64 << 24);
    let mut w = Writer::new();
    w.meta("general.architecture", Value::Str("llama".into()));
    for (k, x) in [
        ("dim", d),
        ("hidden_dim", h),
        ("n_layers", 1),
        ("n_heads", 32),
        ("n_kv_heads", 32),
        ("vocab_size", v),
        ("seq_len", seq),
    ] {
        w.meta(&format!("llama.{k}"), Value::U64(x));
    }
    let mut f = |n: &str, dims: &[u64]| {
        let len = dims.iter().product::<u64>() as usize;
        w.tensor_f32(n, dims, &vec![0.01; len]).unwrap();
    };
    f("tok_embeddings.weight", &[v, d]);
    f("norm.weight", &[d]);
    for (n, dims) in [
        ("attention_norm", vec![d]),
        ("attention.wq", vec![d, d]),
        ("attention.wk", vec![d, d]),
        ("attention.wv", vec![d, d]),
        ("attention.wo", vec![d, d]),
        ("ffn_norm", vec![d]),
        ("feed_forward.w1", vec![h, d]),
        ("feed_forward.w2", vec![d, h]),
        ("feed_forward.w3", vec![h, d]),
    ] {
        f(&format!("layers.0.{n}.weight"), &dims);
    }
    let m = Model::from_vec(w.to_bytes(), true).unwrap();
    let mut t = Transformer::load(&m, 1, 5).unwrap();
    assert_eq!(t.cfg.seq_len, 5);
    if let Ok(tok) = Tokenizer::from_model(&m, 4) {
        let (n, _) = generate(&mut t, &tok, &mut Sampler::new(0.0, 0.9, 1), "", 50, |_| {});
        assert!(n <= 5);
    }
    for i in 0..5 {
        assert!(t.forward(0, i).iter().all(|x| x.is_finite()));
    }
}
