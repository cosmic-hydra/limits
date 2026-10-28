//! The XODFe container: types, validating reader, writer. See SPEC.md.

use crate::error::{Error, Result};
use crate::mmap::Bytes;
use crate::num::{bf16_to_f32, crc32, f16_to_f32};
use std::collections::HashSet;
use std::path::Path;

pub const MAGIC: [u8; 8] = *b"XODFe\0\0\0";
pub const VERSION: u32 = 1;
pub const ALIGN: u64 = 64;
pub const MAX_RANK: usize = 8;
/// Offset of the header (after preamble and header_len).
pub const HEADER_START: u64 = 24;
pub const QK: u64 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DType {
    F32 = 0,
    F16 = 1,
    BF16 = 2,
    I8 = 3,
    Q8_0 = 4,
    Q4_0 = 5,
}

impl DType {
    pub fn from_tag(t: u8) -> Result<DType> {
        Ok(match t {
            0 => DType::F32,
            1 => DType::F16,
            2 => DType::BF16,
            3 => DType::I8,
            4 => DType::Q8_0,
            5 => DType::Q4_0,
            _ => return Err(Error::BadDType(t)),
        })
    }
    pub fn name(self) -> &'static str {
        ["f32", "f16", "bf16", "i8", "q8_0", "q4_0"][self as usize]
    }
    pub fn parse(s: &str) -> Option<DType> {
        (0..6)
            .map(|t| DType::from_tag(t).unwrap())
            .find(|d| d.name() == s)
    }
    pub fn is_quantized(self) -> bool {
        matches!(self, DType::Q8_0 | DType::Q4_0)
    }
    /// Exact byte length for `dims`, enforcing the block shape rule.
    pub fn byte_len(self, name: &str, dims: &[u64]) -> Result<u64> {
        let ovf = || Error::DimOverflow(name.to_string());
        let n = dims
            .iter()
            .try_fold(1u64, |a, &d| a.checked_mul(d))
            .ok_or_else(ovf)?;
        let (num, den) = match self {
            DType::F32 => (4, 1),
            DType::F16 | DType::BF16 => (2, 1),
            DType::I8 => (1, 1),
            DType::Q8_0 => (34, QK),
            DType::Q4_0 => (18, QK),
        };
        if self.is_quantized() && dims.last().is_none_or(|d| d % QK != 0) {
            return Err(Error::BadBlockShape(name.to_string()));
        }
        (n / den).checked_mul(num).ok_or_else(ovf)
    }
}

/// A typed metadata value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    U64(u64),
    I64(i64),
    F64(f64),
    Bool(bool),
    Str(String),
    Bytes(Vec<u8>),
    F32s(Vec<f32>),
    Strs(Vec<String>),
}

impl Value {
    fn tag(&self) -> u8 {
        match self {
            Value::U64(_) => 0,
            Value::I64(_) => 1,
            Value::F64(_) => 2,
            Value::Bool(_) => 3,
            Value::Str(_) => 4,
            Value::Bytes(_) => 5,
            Value::F32s(_) => 6,
            Value::Strs(_) => 7,
        }
    }
    /// Short human-readable rendering (long arrays are summarised).
    pub fn summary(&self) -> String {
        match self {
            Value::U64(v) => format!("{v}"),
            Value::I64(v) => format!("{v}"),
            Value::F64(v) => format!("{v}"),
            Value::Bool(v) => format!("{v}"),
            Value::Str(s) if s.len() > 60 => format!(
                "{:?}... ({} bytes)",
                &s[..s.floor_char_boundary(60)],
                s.len()
            ),
            Value::Str(s) => format!("{s:?}"),
            Value::Bytes(b) => format!("<{} bytes>", b.len()),
            Value::F32s(v) => format!("[f32; {}]", v.len()),
            Value::Strs(v) => format!("[str; {}]", v.len()),
        }
    }
}

/// A validated tensor entry. `offset` is relative to the data section.
#[derive(Debug, Clone, PartialEq)]
pub struct TensorInfo {
    pub name: String,
    pub dtype: DType,
    pub dims: Vec<u64>,
    pub offset: u64,
    pub nbytes: u64,
    pub crc: u32,
}

impl TensorInfo {
    pub fn numel(&self) -> u64 {
        self.dims.iter().product()
    }
}

/// Raw, unvalidated tensor entry used by the low-level encoder.
#[derive(Debug, Clone)]
pub struct RawTensor {
    pub name: String,
    pub dtype: u8,
    pub dims: Vec<u64>,
    pub offset: u64,
    pub nbytes: u64,
    pub crc: u32,
}

// ---------------------------------------------------------------- encoding

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn put_value(out: &mut Vec<u8>, v: &Value) {
    out.push(v.tag());
    match v {
        Value::U64(x) => out.extend_from_slice(&x.to_le_bytes()),
        Value::I64(x) => out.extend_from_slice(&x.to_le_bytes()),
        Value::F64(x) => out.extend_from_slice(&x.to_le_bytes()),
        Value::Bool(b) => out.push(*b as u8),
        Value::Str(s) => put_str(out, s),
        Value::Bytes(b) => {
            out.extend_from_slice(&(b.len() as u64).to_le_bytes());
            out.extend_from_slice(b);
        }
        Value::F32s(v) => {
            out.extend_from_slice(&(v.len() as u64).to_le_bytes());
            v.iter()
                .for_each(|f| out.extend_from_slice(&f.to_le_bytes()));
        }
        Value::Strs(v) => {
            out.extend_from_slice(&(v.len() as u64).to_le_bytes());
            v.iter().for_each(|s| put_str(out, s));
        }
    }
}

/// Encode the header bytes (including the trailing header CRC).
pub fn encode_header(meta: &[(String, Value)], tensors: &[RawTensor]) -> Vec<u8> {
    let mut h = Vec::new();
    h.extend_from_slice(&(meta.len() as u32).to_le_bytes());
    for (k, v) in meta {
        put_str(&mut h, k);
        put_value(&mut h, v);
    }
    h.extend_from_slice(&(tensors.len() as u32).to_le_bytes());
    for t in tensors {
        put_str(&mut h, &t.name);
        h.push(t.dtype);
        h.push(t.dims.len() as u8);
        t.dims
            .iter()
            .for_each(|d| h.extend_from_slice(&d.to_le_bytes()));
        h.extend_from_slice(&t.offset.to_le_bytes());
        h.extend_from_slice(&t.nbytes.to_le_bytes());
        h.extend_from_slice(&t.crc.to_le_bytes());
    }
    let c = crc32(&h);
    h.extend_from_slice(&c.to_le_bytes());
    h
}

pub fn align_up(x: u64) -> u64 {
    x.div_ceil(ALIGN) * ALIGN
}

/// Low-level: preamble + header_len + header + zero padding + `data`.
/// Performs no validation (tests use it to build malformed files).
pub fn encode_raw(
    flags: u32,
    meta: &[(String, Value)],
    tensors: &[RawTensor],
    data: &[u8],
) -> Vec<u8> {
    let header = encode_header(meta, tensors);
    let mut out = Vec::with_capacity(header.len() + data.len() + 128);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&(header.len() as u64).to_le_bytes());
    out.extend_from_slice(&header);
    out.resize(align_up(out.len() as u64) as usize, 0);
    out.extend_from_slice(data);
    out
}

/// Builds a valid XODFe file.
#[derive(Default)]
pub struct Writer {
    meta: Vec<(String, Value)>,
    tensors: Vec<(String, DType, Vec<u64>, Vec<u8>)>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn meta(&mut self, key: &str, v: Value) -> &mut Self {
        self.meta.retain(|(k, _)| k != key);
        self.meta.push((key.to_string(), v));
        self
    }
    pub fn tensor(
        &mut self,
        name: &str,
        dtype: DType,
        dims: &[u64],
        data: Vec<u8>,
    ) -> Result<&mut Self> {
        if dims.len() > MAX_RANK {
            return Err(Error::RankTooLarge {
                name: name.into(),
                rank: dims.len() as u8,
            });
        }
        let expected = dtype.byte_len(name, dims)?;
        if expected != data.len() as u64 {
            return Err(Error::SizeMismatch {
                name: name.into(),
                expected,
                got: data.len() as u64,
            });
        }
        if self.tensors.iter().any(|t| t.0 == name) {
            return Err(Error::DuplicateTensor(name.into()));
        }
        self.tensors.push((name.into(), dtype, dims.to_vec(), data));
        Ok(self)
    }
    pub fn tensor_f32(&mut self, name: &str, dims: &[u64], v: &[f32]) -> Result<&mut Self> {
        self.tensor(
            name,
            DType::F32,
            dims,
            v.iter().flat_map(|f| f.to_le_bytes()).collect(),
        )
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut raws = Vec::new();
        let mut off = 0u64;
        for (name, dt, dims, data) in &self.tensors {
            if !data.is_empty() {
                off = align_up(off);
            }
            raws.push(RawTensor {
                name: name.clone(),
                dtype: *dt as u8,
                dims: dims.clone(),
                // Empty tensors sit at offset 0 so they never point past the end.
                offset: if data.is_empty() { 0 } else { off },
                nbytes: data.len() as u64,
                crc: crc32(data),
            });
            off += data.len() as u64;
        }
        let mut data = vec![0u8; off as usize];
        for (r, (_, _, _, d)) in raws.iter().zip(&self.tensors) {
            if d.is_empty() {
                continue;
            }
            data[r.offset as usize..][..d.len()].copy_from_slice(d);
        }
        encode_raw(0, &self.meta, &raws, &data)
    }
    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, self.to_bytes())?;
        Ok(())
    }
}

// ---------------------------------------------------------------- decoding

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
    base: u64, // absolute file offset of b[0], for error messages
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: u64, what: &'static str) -> Result<&'a [u8]> {
        let rest = (self.b.len() - self.p) as u64;
        if n > rest {
            return Err(Error::Truncated {
                what,
                offset: self.base + self.p as u64,
            });
        }
        let s = &self.b[self.p..self.p + n as usize];
        self.p += n as usize;
        Ok(s)
    }
    fn arr<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N]> {
        Ok(self.take(N as u64, what)?.try_into().unwrap())
    }
    fn u8(&mut self, w: &'static str) -> Result<u8> {
        Ok(self.arr::<1>(w)?[0])
    }
    fn u32(&mut self, w: &'static str) -> Result<u32> {
        Ok(u32::from_le_bytes(self.arr(w)?))
    }
    fn u64(&mut self, w: &'static str) -> Result<u64> {
        Ok(u64::from_le_bytes(self.arr(w)?))
    }
    fn str(&mut self, w: &'static str) -> Result<String> {
        let n = self.u32(w)?;
        let at = self.base + self.p as u64;
        let b = self.take(n as u64, w)?;
        String::from_utf8(b.to_vec()).map_err(|_| Error::BadUtf8 { offset: at })
    }
    fn count(&mut self, elem: u64, w: &'static str) -> Result<u64> {
        // Reject counts that cannot fit in the remaining bytes before allocating.
        let n = self.u64(w)?;
        let rest = (self.b.len() - self.p) as u64;
        if n.checked_mul(elem).is_none_or(|b| b > rest) {
            return Err(Error::Truncated {
                what: w,
                offset: self.base + self.p as u64,
            });
        }
        Ok(n)
    }
    fn value(&mut self) -> Result<Value> {
        Ok(match self.u8("value tag")? {
            0 => Value::U64(self.u64("u64")?),
            1 => Value::I64(i64::from_le_bytes(self.arr("i64")?)),
            2 => Value::F64(f64::from_le_bytes(self.arr("f64")?)),
            3 => match self.u8("bool")? {
                b @ 0..=1 => Value::Bool(b == 1),
                b => return Err(Error::BadBool(b)),
            },
            4 => Value::Str(self.str("str")?),
            5 => {
                let n = self.count(1, "bytes")?;
                Value::Bytes(self.take(n, "bytes")?.to_vec())
            }
            6 => {
                let n = self.count(4, "[f32]")?;
                let b = self.take(n * 4, "[f32]")?;
                Value::F32s(
                    b.chunks_exact(4)
                        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                )
            }
            7 => {
                let n = self.count(4, "[str]")?;
                let mut v = Vec::new();
                for _ in 0..n {
                    v.push(self.str("[str]")?);
                }
                Value::Strs(v)
            }
            t => return Err(Error::BadValueTag(t)),
        })
    }
}

/// Fully parsed, validated header.
#[derive(Debug, Clone)]
pub struct Header {
    pub flags: u32,
    pub meta: Vec<(String, Value)>,
    pub tensors: Vec<TensorInfo>,
    /// Absolute file offset of the data section.
    pub data_start: u64,
}

/// Parse and validate everything except per-tensor CRCs. Never panics.
pub fn parse(file: &[u8]) -> Result<Header> {
    let mut c = Cur {
        b: file,
        p: 0,
        base: 0,
    };
    if c.take(8, "magic")? != MAGIC {
        return Err(Error::BadMagic);
    }
    let version = c.u32("version")?;
    if version != VERSION {
        return Err(Error::UnsupportedVersion(version));
    }
    let flags = c.u32("flags")?;
    if flags != 0 {
        return Err(Error::UnknownFlags(flags));
    }
    let hlen = c.u64("header_len")?;
    let flen = file.len() as u64;
    let hend = HEADER_START
        .checked_add(hlen)
        .ok_or(Error::BadHeaderLen(hlen))?;
    if hlen < 12 || hend > flen {
        return Err(Error::BadHeaderLen(hlen));
    }
    let hbytes = &file[HEADER_START as usize..hend as usize];
    let (body, crc_bytes) = hbytes.split_at(hbytes.len() - 4);
    let stored = u32::from_le_bytes(crc_bytes.try_into().unwrap());
    let computed = crc32(body);
    if stored != computed {
        return Err(Error::HeaderCrc { stored, computed });
    }

    let mut h = Cur {
        b: body,
        p: 0,
        base: HEADER_START,
    };
    let mut meta = Vec::new();
    let mut keys = HashSet::new();
    for _ in 0..h.u32("metadata count")? {
        let k = h.str("metadata key")?;
        let v = h.value()?;
        if !keys.insert(k.clone()) {
            return Err(Error::DuplicateKey(k));
        }
        meta.push((k, v));
    }
    let mut tensors = Vec::new();
    let mut names = HashSet::new();
    for _ in 0..h.u32("tensor count")? {
        let name = h.str("tensor name")?;
        let dtype = DType::from_tag(h.u8("dtype")?)?;
        let rank = h.u8("rank")?;
        if rank as usize > MAX_RANK {
            return Err(Error::RankTooLarge { name, rank });
        }
        let mut dims = Vec::with_capacity(rank as usize);
        for _ in 0..rank {
            dims.push(h.u64("dim")?);
        }
        let offset = h.u64("offset")?;
        let nbytes = h.u64("nbytes")?;
        let crc = h.u32("tensor crc")?;
        let expected = dtype.byte_len(&name, &dims)?;
        if nbytes != expected {
            return Err(Error::SizeMismatch {
                name,
                expected,
                got: nbytes,
            });
        }
        if offset % ALIGN != 0 {
            return Err(Error::Misaligned { name, offset });
        }
        if !names.insert(name.clone()) {
            return Err(Error::DuplicateTensor(name));
        }
        tensors.push(TensorInfo {
            name,
            dtype,
            dims,
            offset,
            nbytes,
            crc,
        });
    }
    if h.p != body.len() {
        return Err(Error::TrailingHeaderBytes((body.len() - h.p) as u64));
    }

    let data_start = align_up(hend);
    if data_start > flen {
        return Err(Error::Truncated {
            what: "header padding",
            offset: flen,
        });
    }
    if file[hend as usize..data_start as usize]
        .iter()
        .any(|&b| b != 0)
    {
        return Err(Error::NonZeroPadding);
    }
    let data_len = flen - data_start;
    let mut order: Vec<&TensorInfo> = tensors.iter().collect();
    order.sort_by_key(|t| t.offset);
    let mut end = 0u64;
    let mut prev: Option<&TensorInfo> = None;
    for t in order {
        let tend = t.offset.checked_add(t.nbytes).filter(|&e| e <= data_len);
        let tend = tend.ok_or_else(|| Error::OutOfBounds {
            name: t.name.clone(),
        })?;
        if t.nbytes == 0 {
            continue;
        }
        if let Some(p) = prev {
            if t.offset < end {
                return Err(Error::Overlap {
                    a: p.name.clone(),
                    b: t.name.clone(),
                });
            }
        }
        end = tend;
        prev = Some(t);
    }
    if data_len != end {
        return Err(Error::TrailingData(data_len - end));
    }
    Ok(Header {
        flags,
        meta,
        tensors,
        data_start,
    })
}

/// An opened XODFe model: validated header plus (mapped) bytes.
pub struct Model {
    bytes: Bytes,
    pub header: Header,
}

impl Model {
    /// Open and validate a file (memory-mapped where possible).
    /// With `verify_crc`, every tensor checksum is checked too.
    pub fn open(path: impl AsRef<Path>, verify_crc: bool) -> Result<Model> {
        let mut f = std::fs::File::open(path)?;
        Self::from_bytes(Bytes::map(&mut f)?, verify_crc)
    }
    pub fn from_vec(v: Vec<u8>, verify_crc: bool) -> Result<Model> {
        Self::from_bytes(Bytes::Owned(v), verify_crc)
    }
    fn from_bytes(bytes: Bytes, verify_crc: bool) -> Result<Model> {
        let header = parse(&bytes)?;
        let m = Model { bytes, header };
        if verify_crc {
            m.verify_checksums()?;
        }
        Ok(m)
    }
    pub fn verify_checksums(&self) -> Result<()> {
        for t in &self.header.tensors {
            let computed = crc32(self.data(t));
            if computed != t.crc {
                return Err(Error::TensorCrc {
                    name: t.name.clone(),
                    stored: t.crc,
                    computed,
                });
            }
        }
        Ok(())
    }
    pub fn file_len(&self) -> u64 {
        self.bytes.len() as u64
    }
    pub fn tensors(&self) -> &[TensorInfo] {
        &self.header.tensors
    }
    pub fn tensor(&self, name: &str) -> Option<&TensorInfo> {
        self.header.tensors.iter().find(|t| t.name == name)
    }
    /// Raw bytes of a tensor (bounds were validated at open).
    pub fn data(&self, t: &TensorInfo) -> &[u8] {
        let s = (self.header.data_start + t.offset) as usize;
        &self.bytes[s..s + t.nbytes as usize]
    }
    pub fn meta(&self, key: &str) -> Option<&Value> {
        self.header
            .meta
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }
    pub fn meta_u64(&self, key: &str) -> Result<u64> {
        match self.meta(key) {
            Some(Value::U64(v)) => Ok(*v),
            _ => crate::error::fail(format!("missing or non-u64 metadata {key:?}")),
        }
    }
    pub fn meta_str(&self, key: &str) -> Option<&str> {
        match self.meta(key) {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        }
    }
    /// Decode any tensor to f32 (dequantizing as needed).
    pub fn to_f32(&self, t: &TensorInfo) -> Vec<f32> {
        decode_f32(t.dtype, self.data(t))
    }
}

/// Decode `data` of the given dtype into f32 values.
pub fn decode_f32(dtype: DType, data: &[u8]) -> Vec<f32> {
    let u16s = |d: &[u8]| {
        d.chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect::<Vec<_>>()
    };
    match dtype {
        DType::F32 => data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect(),
        DType::F16 => u16s(data).into_iter().map(f16_to_f32).collect(),
        DType::BF16 => u16s(data).into_iter().map(bf16_to_f32).collect(),
        DType::I8 => data.iter().map(|&b| b as i8 as f32).collect(),
        DType::Q8_0 => data
            .chunks_exact(34)
            .flat_map(|b| {
                let d = f16_to_f32(u16::from_le_bytes([b[0], b[1]]));
                b[2..].iter().map(move |&q| q as i8 as f32 * d)
            })
            .collect(),
        DType::Q4_0 => data
            .chunks_exact(18)
            .flat_map(|b| {
                let d = f16_to_f32(u16::from_le_bytes([b[0], b[1]]));
                let lo = b[2..]
                    .iter()
                    .map(move |&q| ((q & 0xf) as i32 - 8) as f32 * d);
                let hi = b[2..]
                    .iter()
                    .map(move |&q| ((q >> 4) as i32 - 8) as f32 * d);
                lo.chain(hi)
            })
            .collect(),
    }
}
