//! XODFe: a single-file, validated model format (tensors + typed metadata +
//! tokenizer) for the Limits project. Zero dependencies. See SPEC.md.

pub mod error;
pub mod format;
pub mod json;
pub mod llama;
pub mod llama2c;
pub mod mmap;
pub mod num;
pub mod quant;
pub mod safetensors;

pub use error::{Error, Result};
pub use format::{DType, Model, TensorInfo, Value, Writer};
