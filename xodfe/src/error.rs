//! Typed errors. Every failure the reader can detect has its own variant so
//! callers (and tests) can match on the exact cause.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// Underlying I/O failure (message of the `std::io::Error`).
    Io(String),
    /// The first 8 bytes are not `XODFe\0\0\0`.
    BadMagic,
    UnsupportedVersion(u32),
    /// Flag bits this reader does not understand were set.
    UnknownFlags(u32),
    /// Input ended while reading `what` at byte `offset`.
    Truncated {
        what: &'static str,
        offset: u64,
    },
    /// `header_len` is too small or runs past the end of the file.
    BadHeaderLen(u64),
    HeaderCrc {
        stored: u32,
        computed: u32,
    },
    /// Bytes left over inside the header after the last entry.
    TrailingHeaderBytes(u64),
    /// Padding between header and data section is not zero.
    NonZeroPadding,
    BadUtf8 {
        offset: u64,
    },
    BadValueTag(u8),
    BadBool(u8),
    DuplicateKey(String),
    BadDType(u8),
    RankTooLarge {
        name: String,
        rank: u8,
    },
    /// Product of dims (or byte size) overflows u64.
    DimOverflow(String),
    /// Quantized tensors need rank >= 1 and a last dim divisible by 32.
    BadBlockShape(String),
    SizeMismatch {
        name: String,
        expected: u64,
        got: u64,
    },
    Misaligned {
        name: String,
        offset: u64,
    },
    OutOfBounds {
        name: String,
    },
    Overlap {
        a: String,
        b: String,
    },
    DuplicateTensor(String),
    TensorCrc {
        name: String,
        stored: u32,
        computed: u32,
    },
    /// Bytes after the end of the last tensor.
    TrailingData(u64),
    /// Errors from the importers/exporters (safetensors, llama2.c) and CLI.
    Format(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Error::*;
        match self {
            Io(m) => write!(f, "I/O error: {m}"),
            BadMagic => write!(f, "not an XODFe file (bad magic)"),
            UnsupportedVersion(v) => {
                write!(f, "unsupported XODFe version {v} (this reader supports 1)")
            }
            UnknownFlags(x) => write!(f, "unknown flag bits set: {x:#x}"),
            Truncated { what, offset } => {
                write!(f, "truncated input reading {what} at byte {offset}")
            }
            BadHeaderLen(n) => write!(f, "invalid header_len {n}"),
            HeaderCrc { stored, computed } => {
                write!(
                    f,
                    "header checksum mismatch (stored {stored:08x}, computed {computed:08x})"
                )
            }
            TrailingHeaderBytes(n) => write!(f, "{n} unexpected trailing bytes in header"),
            NonZeroPadding => write!(f, "header padding contains non-zero bytes"),
            BadUtf8 { offset } => write!(f, "invalid UTF-8 in string at byte {offset}"),
            BadValueTag(t) => write!(f, "unknown metadata value tag {t}"),
            BadBool(b) => write!(f, "invalid bool byte {b}"),
            DuplicateKey(k) => write!(f, "duplicate metadata key {k:?}"),
            BadDType(t) => write!(f, "unknown dtype tag {t}"),
            RankTooLarge { name, rank } => write!(f, "tensor {name:?}: rank {rank} exceeds 8"),
            DimOverflow(n) => write!(f, "tensor {n:?}: dimensions overflow"),
            BadBlockShape(n) => write!(
                f,
                "tensor {n:?}: quantized tensors need last dim divisible by 32"
            ),
            SizeMismatch {
                name,
                expected,
                got,
            } => {
                write!(
                    f,
                    "tensor {name:?}: byte length {got} != expected {expected}"
                )
            }
            Misaligned { name, offset } => {
                write!(f, "tensor {name:?}: offset {offset} not 64-byte aligned")
            }
            OutOfBounds { name } => {
                write!(f, "tensor {name:?}: data lies outside the data section")
            }
            Overlap { a, b } => write!(f, "tensors {a:?} and {b:?} overlap"),
            DuplicateTensor(n) => write!(f, "duplicate tensor name {n:?}"),
            TensorCrc {
                name,
                stored,
                computed,
            } => write!(
                f,
                "tensor {name:?}: checksum mismatch (stored {stored:08x}, computed {computed:08x})"
            ),
            TrailingData(n) => write!(f, "{n} unexpected bytes after the last tensor"),
            Format(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Shorthand for `Err(Error::Format(..))`.
pub fn fail<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Format(msg.into()))
}
