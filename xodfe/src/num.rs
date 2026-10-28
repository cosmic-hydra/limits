//! Dependency-free numeric helpers: CRC-32 (IEEE) and f16/bf16 conversion.

const fn crc_tables() -> [[u32; 256]; 8] {
    let mut t = [[0u32; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[0][i] = c;
        i += 1;
    }
    let mut i = 0;
    while i < 256 {
        let mut s = 1;
        while s < 8 {
            let p = t[s - 1][i];
            t[s][i] = (p >> 8) ^ t[0][(p & 0xff) as usize];
            s += 1;
        }
        i += 1;
    }
    t
}

static CRC: [[u32; 256]; 8] = crc_tables();

/// CRC-32/ISO-HDLC (same as zlib `crc32`), slicing-by-8.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    let mut chunks = data.chunks_exact(8);
    for b in &mut chunks {
        let lo = c ^ u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        c = CRC[7][(lo & 0xff) as usize]
            ^ CRC[6][((lo >> 8) & 0xff) as usize]
            ^ CRC[5][((lo >> 16) & 0xff) as usize]
            ^ CRC[4][(lo >> 24) as usize]
            ^ CRC[3][b[4] as usize]
            ^ CRC[2][b[5] as usize]
            ^ CRC[1][b[6] as usize]
            ^ CRC[0][b[7] as usize];
    }
    for &b in chunks.remainder() {
        c = CRC[0][((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// IEEE 754 binary16 -> f32 (exact).
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1f) as u32;
    let man = (h & 0x3ff) as u32;
    let bits = match (exp, man) {
        (0, 0) => sign,
        (0, _) => {
            // Subnormal: value = man * 2^-24; normalise.
            let v = man as f32 * (1.0 / 16_777_216.0);
            return if sign != 0 { -v } else { v };
        }
        (31, 0) => sign | 0x7f80_0000,
        (31, _) => sign | 0x7fc0_0000 | (man << 13),
        _ => sign | ((exp + 112) << 23) | (man << 13),
    };
    f32::from_bits(bits)
}

/// f32 -> binary16 with round-to-nearest-even; overflow saturates to inf.
pub fn f32_to_f16(f: f32) -> u16 {
    let x = f.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32;
    let man = x & 0x7f_ffff;
    if exp == 255 {
        return sign
            | 0x7c00
            | if man != 0 {
                0x200 | (man >> 13) as u16
            } else {
                0
            };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign; // underflows to signed zero
        }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let rem = m & ((1 << shift) - 1);
        let mut r = m >> shift;
        if rem > half || (rem == half && r & 1 == 1) {
            r += 1;
        }
        return sign | r as u16;
    }
    let mut r = ((e as u32) << 10) | (man >> 13);
    let rem = man & 0x1fff;
    if rem > 0x1000 || (rem == 0x1000 && r & 1 == 1) {
        r += 1; // may carry into the exponent, which is the correct result
    }
    sign | r as u16
}

pub fn bf16_to_f32(h: u16) -> f32 {
    f32::from_bits((h as u32) << 16)
}

/// f32 -> bfloat16 with round-to-nearest-even; NaN stays NaN.
pub fn f32_to_bf16(f: f32) -> u16 {
    let x = f.to_bits();
    if f.is_nan() {
        return ((x >> 16) | 0x40) as u16;
    }
    let round = 0x7fff + ((x >> 16) & 1);
    (x.wrapping_add(round) >> 16) as u16
}
