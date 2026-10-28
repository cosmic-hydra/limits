//! safetensors import/export (format: u64 LE header length, JSON header, data).

use crate::error::{fail, Result};
use crate::format::{DType, Model, Value, Writer};
use crate::json::{self, quote, Json};

/// Metadata keys from `__metadata__` are stored under this prefix.
pub const META_PREFIX: &str = "safetensors.";
const MAX_HEADER: u64 = 100 << 20;

fn st_dtype(s: &str) -> Result<DType> {
    Ok(match s {
        "F32" => DType::F32,
        "F16" => DType::F16,
        "BF16" => DType::BF16,
        "I8" => DType::I8,
        other => {
            return fail(format!(
                "safetensors dtype {other} is not supported by XODFe"
            ))
        }
    })
}

/// Parse a safetensors file into an XODFe writer. Tensors keep file order.
pub fn import(file: &[u8]) -> Result<Writer> {
    let Some(len) = file.get(..8) else {
        return fail("safetensors: file shorter than 8 bytes");
    };
    let n = u64::from_le_bytes(len.try_into().unwrap());
    if n > MAX_HEADER || 8 + n > file.len() as u64 {
        return fail(format!("safetensors: bad header length {n}"));
    }
    let data = &file[8 + n as usize..];
    let Json::Obj(entries) = json::parse(&file[8..8 + n as usize])? else {
        return fail("safetensors: header is not a JSON object");
    };
    let mut w = Writer::new();
    let mut list = Vec::new();
    for (name, v) in &entries {
        if name == "__metadata__" {
            let Json::Obj(kv) = v else {
                return fail("safetensors: __metadata__ must be an object");
            };
            for (k, v) in kv {
                let Some(s) = v.as_str() else {
                    return fail("safetensors: metadata values must be strings");
                };
                w.meta(&format!("{META_PREFIX}{k}"), Value::Str(s.to_string()));
            }
            continue;
        }
        let bad = || fail(format!("safetensors: malformed entry {name:?}"));
        let Some(dt) = v.get("dtype").and_then(Json::as_str) else {
            return bad();
        };
        let Some(Json::Arr(shape)) = v.get("shape") else {
            return bad();
        };
        let Some(Json::Arr(offs)) = v.get("data_offsets") else {
            return bad();
        };
        let dims: Option<Vec<u64>> = shape.iter().map(Json::as_u64).collect();
        let Some(dims) = dims else { return bad() };
        let (Some(b), Some(e)) = (
            offs.first().and_then(Json::as_u64),
            offs.get(1).and_then(Json::as_u64),
        ) else {
            return bad();
        };
        if offs.len() != 2 || b > e || e > data.len() as u64 {
            return fail(format!(
                "safetensors: tensor {name:?} has out-of-range offsets"
            ));
        }
        list.push((b, name, st_dtype(dt)?, dims, e));
    }
    // Like the reference loader: tensor byte ranges must tile the data
    // buffer exactly (no overlaps, no gaps, no trailing bytes).
    list.sort_by_key(|t| (t.0, t.4));
    let mut end = 0u64;
    for (b, name, _, _, e) in &list {
        if *b != end {
            return fail(format!(
                "safetensors: tensor {name:?} starts at {b}, expected {end} (overlap or gap)"
            ));
        }
        end = *e;
    }
    if end != data.len() as u64 {
        return fail(format!(
            "safetensors: {} data bytes not covered by any tensor",
            data.len() as u64 - end
        ));
    }
    for (b, name, dt, dims, e) in list {
        w.tensor(name, dt, &dims, data[b as usize..e as usize].to_vec())?;
    }
    Ok(w)
}

/// Export an XODFe model to safetensors bytes (f32/f16/bf16 tensors only).
/// String metadata under `safetensors.` goes back into `__metadata__`;
/// other metadata (e.g. tokenizer) has no safetensors equivalent and is dropped.
pub fn export(m: &Model) -> Result<Vec<u8>> {
    let mut entries = Vec::new();
    let meta: Vec<String> = m
        .header
        .meta
        .iter()
        .filter_map(|(k, v)| match (k.strip_prefix(META_PREFIX), v) {
            (Some(k), Value::Str(s)) => Some(format!("{}:{}", quote(k), quote(s))),
            _ => None,
        })
        .collect();
    if !meta.is_empty() {
        entries.push(format!("\"__metadata__\":{{{}}}", meta.join(",")));
    }
    let mut off = 0u64;
    for t in m.tensors() {
        let dt = match t.dtype {
            DType::F32 => "F32",
            DType::F16 => "F16",
            DType::BF16 => "BF16",
            d => {
                return fail(format!(
                    "tensor {:?}: dtype {} cannot be exported to safetensors",
                    t.name,
                    d.name()
                ))
            }
        };
        let shape: Vec<String> = t.dims.iter().map(u64::to_string).collect();
        entries.push(format!(
            "{}:{{\"dtype\":\"{dt}\",\"shape\":[{}],\"data_offsets\":[{off},{}]}}",
            quote(&t.name),
            shape.join(","),
            off + t.nbytes
        ));
        off += t.nbytes;
    }
    let mut header = format!("{{{}}}", entries.join(",")).into_bytes();
    header.resize(header.len().div_ceil(8) * 8, b' ');
    let mut out = Vec::with_capacity(8 + header.len() + off as usize);
    out.extend_from_slice(&(header.len() as u64).to_le_bytes());
    out.extend_from_slice(&header);
    for t in m.tensors() {
        out.extend_from_slice(m.data(t));
    }
    Ok(out)
}
