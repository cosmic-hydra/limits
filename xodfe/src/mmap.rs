//! Read-only file bytes: a memory map on Linux, a `Vec<u8>` elsewhere or when
//! mapping fails. This is the only module containing `unsafe`.

use std::fs::File;
use std::ops::Deref;

pub enum Bytes {
    #[cfg(target_os = "linux")]
    Mapped {
        ptr: *const u8,
        len: usize,
    },
    Owned(Vec<u8>),
}

// SAFETY: the mapping is private and read-only, never mutated or aliased mutably,
// so sharing the pointer across threads is as safe as sharing a `&[u8]`.
unsafe impl Send for Bytes {}
unsafe impl Sync for Bytes {}

#[cfg(target_os = "linux")]
mod sys {
    use std::ffi::c_void;
    pub const PROT_READ: i32 = 1;
    pub const MAP_PRIVATE: i32 = 2;
    extern "C" {
        pub fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: i32,
            flags: i32,
            fd: i32,
            off: i64,
        ) -> *mut c_void;
        pub fn munmap(addr: *mut c_void, len: usize) -> i32;
    }
}

impl Bytes {
    /// Map `file` read-only. Falls back to reading it fully into memory.
    /// Note: like every mmap-based loader, a mapped file that another process
    /// truncates while in use can raise SIGBUS; use `Owned` if that matters.
    pub fn map(file: &mut File) -> std::io::Result<Bytes> {
        let len = file.metadata()?.len();
        #[cfg(target_os = "linux")]
        if len > 0 {
            if let Ok(len) = usize::try_from(len) {
                use std::os::fd::AsRawFd;
                // SAFETY: valid fd, len > 0, read-only private mapping; result checked.
                let p = unsafe {
                    sys::mmap(
                        std::ptr::null_mut(),
                        len,
                        sys::PROT_READ,
                        sys::MAP_PRIVATE,
                        file.as_raw_fd(),
                        0,
                    )
                };
                if p as isize != -1 {
                    return Ok(Bytes::Mapped {
                        ptr: p as *const u8,
                        len,
                    });
                }
            }
        }
        let mut v = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
        std::io::Read::read_to_end(file, &mut v)?;
        Ok(Bytes::Owned(v))
    }
}

impl Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            // SAFETY: ptr/len come from a successful mmap that lives until Drop.
            #[cfg(target_os = "linux")]
            Bytes::Mapped { ptr, len } => unsafe { std::slice::from_raw_parts(*ptr, *len) },
            Bytes::Owned(v) => v,
        }
    }
}

impl Drop for Bytes {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if let Bytes::Mapped { ptr, len } = *self {
            // SAFETY: unmapping exactly the region we mapped, once.
            unsafe { sys::munmap(ptr as *mut _, len) };
        }
    }
}
