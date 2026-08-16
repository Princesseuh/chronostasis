//! The "ffxiiicrypt" block cipher guarding XIII-2 and LR `filelist` indexes.

use crate::{FormatError, Result};

/// `0x1DE10F38` at filelist header offset 20 marks an encrypted index.
pub const ENC_TAG: u32 = 501232760;

#[inline]
fn perm(x: u8) -> u8 {
    x.wrapping_add(120)
}

fn generate_xor_table(seed: [u8; 8]) -> [u8; 264] {
    let mut seed = seed;
    seed.reverse();
    let a = u32::from_le_bytes([seed[0], seed[1], seed[2], seed[3]]).rotate_left(8);
    let b = u32::from_le_bytes([seed[4], seed[5], seed[6], seed[7]]).rotate_left(16);

    let mut block = [0u8; 8];
    block[0..4].copy_from_slice(&b.to_le_bytes());
    block[4..8].copy_from_slice(&a.to_le_bytes());
    block[0] = block[0].wrapping_add(69);
    for i in 1..8 {
        let mut t = block[i] as i32 + 212 + block[i - 1] as i32;
        t ^= (block[i - 1] as i32) << 2;
        t ^= 0x45;
        block[i] = t as u8;
    }

    let mut table = [0u8; 264];
    table[0..8].copy_from_slice(&block);
    let mut state = u64::from_le_bytes(block);
    for blk in 1..33 {
        let lo = state as u32;
        let hi = (state >> 32) as u32;
        let mut m = 5u64.wrapping_mul(state);
        m ^= (hi as u64) << 32;
        let n9 = lo ^ (m as u32);
        let n10 = (m >> 32) as u32;
        m = (lo as u64) | (m & 0xFFFF_FFFF_0000_0000);
        let out_lo = (m as u32) ^ n9;
        let out_hi = n10 ^ hi;
        block[0..4].copy_from_slice(&out_lo.to_le_bytes());
        block[4..8].copy_from_slice(&out_hi.to_le_bytes());
        table[blk * 8..blk * 8 + 8].copy_from_slice(&block);
        state = u64::from_le_bytes(block);
    }
    table
}

/// `x` stays in 0..=255 throughout, so the `perm` index is always in range.
fn loop_a_byte(mut x: u32, table: &[u8; 264], off: usize) -> u32 {
    for i in 0..8 {
        let n = perm(x as u8) as i32;
        let t = n - table[off + i] as i32;
        x = if t >= 0 { t as u32 } else { (t as u32) & 0xFF };
    }
    x
}

/// Real block counters keep the 64-bit shifts non-negative, so unsigned arithmetic matches.
fn block_counter_setup(bc: u32) -> (u32, u32) {
    let p10 = (bc as u64) << 10;
    let p20 = (bc as u64) << 20;
    let p30 = (bc as u64) << 30;
    let n5 = p10 as u32;
    let n6 = (p10 >> 32) as u32;
    let n7 = (p20 as u32) | n5;
    let n8 = ((p20 >> 32) as u32) | n6;
    let mut eval = p30 as u32;
    let mut fval = (p30 >> 32) as u32;
    eval |= bc;
    eval |= n7;
    fval |= n8;
    (eval, fval)
}

fn special_keys(eval: u32, fval: u32) -> (i64, i64) {
    let carry = (eval > 1_587_207_352) as i64;
    let key1 = eval as i64 + 2_707_759_943;
    let key2 = fval as i64 + carry;
    (key1, key2)
}

fn decrypt_block(a: [u8; 8], bc: u32, table: &[u8; 264]) -> [u8; 8] {
    let off = (bc & 0xF8) as usize;
    let c2 = bc >> 3;
    let d1 = loop_a_byte(((c2 ^ 0x45) & 0xFF) ^ a[0] as u32, table, off);
    let d2 = loop_a_byte((a[0] ^ a[1]) as u32, table, off);
    let d3 = loop_a_byte((a[1] ^ a[2]) as u32, table, off);
    let d4 = loop_a_byte((a[2] ^ a[3]) as u32, table, off);
    let d5 = loop_a_byte((a[3] ^ a[4]) as u32, table, off);
    let d6 = loop_a_byte((a[4] ^ a[5]) as u32, table, off);
    let d7 = loop_a_byte((a[5] ^ a[6]) as u32, table, off);
    let d8 = loop_a_byte((a[6] ^ a[7]) as u32, table, off);

    let lo = u32::from_le_bytes([d5 as u8, d6 as u8, d7 as u8, d8 as u8]);
    let hi = u32::from_le_bytes([d1 as u8, d2 as u8, d3 as u8, d4 as u8]);
    let xor_lo = u32::from_le_bytes(table[off..off + 4].try_into().unwrap());
    let xor_hi = u32::from_le_bytes(table[off + 4..off + 8].try_into().unwrap());
    let (eval, fval) = block_counter_setup(bc);
    let (key1, key2) = special_keys(eval, fval);

    let mut x = hi as i64;
    let mut y = lo as i64;
    let borrow = (x < xor_lo as i64) as i64;
    x -= xor_lo as i64;
    y -= xor_hi as i64;
    y -= borrow;
    x ^= key1;
    y ^= key2;
    x ^= xor_lo as i64;
    y ^= xor_hi as i64;

    let mut out = [0u8; 8];
    out[0..4].copy_from_slice(&(y as u32).to_le_bytes());
    out[4..8].copy_from_slice(&(x as u32).to_le_bytes());
    out
}

/// Only the `cryptBodySize` body is enciphered; the 32-byte header and trailing padding are copied
/// verbatim, so the result parses like any plaintext filelist.
pub fn decrypt_filelist(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 32 {
        return Err(malformed("too short for an encrypted header"));
    }
    let body =
        u32::from_be_bytes([data[16], data[17], data[18], data[19]]).wrapping_add(8) as usize;
    if !body.is_multiple_of(8) {
        return Err(malformed("encrypted body not a multiple of 8"));
    }
    if 32 + body > data.len() {
        return Err(malformed("encrypted body runs past end of file"));
    }

    // The cipher's seed is signed, so a high bit 31 must fill the high word before widening.
    let packed = ((data[9] as u32) << 24)
        | ((data[12] as u32) << 16)
        | ((data[2] as u32) << 8)
        | data[0] as u32;
    let seed = ((packed as i32) as i64) as u64;
    let table = generate_xor_table(seed.to_le_bytes());

    let mut out = data.to_vec();
    let mut bc = 0u32;
    for i in 0..body / 8 {
        let off = 32 + i * 8;
        let block = decrypt_block(data[off..off + 8].try_into().unwrap(), bc, &table);
        out[off..off + 8].copy_from_slice(&block);
        bc = bc.wrapping_add(8);
    }
    Ok(out)
}

fn malformed(detail: &str) -> FormatError {
    FormatError::Malformed {
        format: "filelist",
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Known-answer vector lifted from the real FFXIII-2 system filelist.
    const HEADER: [u8; 32] = [
        0xf5, 0xb2, 0xca, 0x00, 0xb5, 0x7f, 0xca, 0xc8, 0x57, 0xa1, 0xa4, 0x5c, 0x56, 0x23, 0xec,
        0x60, 0x00, 0x04, 0x49, 0xe0, 0x78, 0x34, 0xe0, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00,
    ];
    const ENC_BLOCK0: [u8; 8] = [0x43, 0x08, 0xe5, 0xd0, 0x3b, 0x66, 0xfe, 0x1a];
    const DEC_BLOCK0: [u8; 8] = [0x74, 0xc1, 0x01, 0x00, 0xdc, 0xc5, 0x01, 0x00];

    #[test]
    fn header_carries_the_tag() {
        let tag = u32::from_le_bytes(HEADER[20..24].try_into().unwrap());
        assert_eq!(tag, ENC_TAG);
    }

    #[test]
    fn decrypts_known_block() {
        let packed = ((HEADER[9] as u32) << 24)
            | ((HEADER[12] as u32) << 16)
            | ((HEADER[2] as u32) << 8)
            | HEADER[0] as u32;
        let seed = ((packed as i32) as i64) as u64;
        let table = generate_xor_table(seed.to_le_bytes());
        assert_eq!(decrypt_block(ENC_BLOCK0, 0, &table), DEC_BLOCK0);
        // The decrypted word is the entries-table extent, an anchor outside the cipher itself.
        assert_eq!(
            u32::from_le_bytes(DEC_BLOCK0[0..4].try_into().unwrap()),
            12 + 14381 * 8
        );
    }
}
