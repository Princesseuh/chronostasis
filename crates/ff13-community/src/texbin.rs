//! Deobfuscation for the HD-mod `.bin` texture files: a stream cipher keyed by texture name.

/// Body-stream perturbation interval (`0x14cb`).
const CHECKPOINT: usize = 5323;

/// Combined xorshift / add-with-carry / Weyl generator the tool keys on.
struct Prng {
    s_a: u32,
    s_b: u32,
    s_c: u32,
    carry: u32,
    weyl: u32,
}

impl Prng {
    /// Seeds from a texture name (cipher key); mixing runs over the full name,
    /// warm-up length is the tool's constant 3.
    fn seed(name: &[u8]) -> Self {
        // case-normalize: even idx upper, odd lower
        let mut k = name.to_vec();
        for (i, c) in k.iter_mut().enumerate() {
            if i & 1 == 1 {
                c.make_ascii_lowercase();
            } else {
                c.make_ascii_uppercase();
            }
        }
        let mixlen = k.len().max(1);
        let mut seeds: [u32; 4] = [0x075b_cd15, 0x0dfb_38d3, 0x149a_a440, 0x1b3a_0c83];
        for i in 0..16u32 {
            let kb = k[i as usize % mixlen] as i8 as i32 as u32;
            let shift = ((i.wrapping_mul(0xaaab) >> 17) % 3) * 7;
            let slot = (i & 3) as usize;
            seeds[slot] = seeds[slot].wrapping_add(kb << shift);
        }
        let mut p = Prng {
            weyl: seeds[0],
            s_c: seeds[1].wrapping_add(1),
            s_b: seeds[2],
            s_a: seeds[3],
            carry: 0,
        };
        // warm-up: tool passes length 3 regardless of key
        let si = (p.next() & 0xf) + 3 + 2;
        for _ in 0..si {
            p.next();
        }
        p
    }

    fn next(&mut self) -> u32 {
        let mut x = self.s_c;
        x ^= x << 5;
        x ^= ((x as i32) >> 7) as u32;
        x ^= x << 22;
        self.s_c = x;
        let summ = self.s_b.wrapping_add(self.s_a).wrapping_add(self.carry);
        let new_a = summ & 0x7fff_ffff;
        self.s_b = self.s_a;
        self.s_a = new_a;
        self.carry = summ >> 31;
        self.weyl = self.weyl.wrapping_add(0x5420_23ab);
        self.weyl.wrapping_add(x).wrapping_add(new_a)
    }
}

/// Deobfuscates a `.bin` body to raw DXT mip-chain bytes. `name` (no extension,
/// = `.bin` basename = GTEX descriptor name) is the cipher key; `data` is the
/// whole `.bin` (8-byte header + body); result is `data.len()-8` bytes. Errors
/// when `name` is empty (no cipher key).
pub fn decode(name: &str, data: &[u8]) -> ff13_formats::Result<Vec<u8>> {
    Ok(decode_full(name, data)?.1)
}

/// Deobfuscates the full `.bin` -> (plaintext 8-byte header, DXT body); header
/// encodes format/width/height/mip-count (see [`TexInfo`]). Same cipher stream.
/// Errors when `name` is empty (no cipher key).
pub fn decode_full(name: &str, data: &[u8]) -> ff13_formats::Result<([u8; 8], Vec<u8>)> {
    if name.is_empty() {
        return Err(ff13_formats::FormatError::Unsupported(
            ".bin texture with an empty name: the file name is the cipher key".into(),
        ));
    }
    let mut p = Prng::seed(name.as_bytes());
    let mut hdr = [0u8; 8];
    for (i, slot) in hdr.iter_mut().enumerate() {
        *slot = data
            .get(i)
            .copied()
            .unwrap_or(0)
            .wrapping_sub(p.next() as u8);
    }
    let body = data.get(8..).unwrap_or(&[]);
    let mut out = Vec::with_capacity(body.len());
    let mut stride: u32 = 3;
    for (i, &c) in body.iter().enumerate() {
        out.push(c.wrapping_sub(p.next() as u8));
        if i % CHECKPOINT == 0 {
            p.weyl = p.weyl.wrapping_add(0xb);
            p.s_c = p.s_c.wrapping_add(0x1f);
            p.s_b = p.s_b.wrapping_add(0x15b);
            let v1 = p.next();
            for _ in 0..stride {
                p.next();
            }
            let v2 = p.next();
            stride = ((((v1 >> 3) & 0xffff) + (v2 >> 17)) & 7) + 3;
        }
    }
    Ok((hdr, out))
}

/// Format/dimensions parsed from a decoded `.bin` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TexInfo {
    /// GTEX format (24=DXT1, 25=DXT3, 26=DXT5).
    pub format: u8,
    pub width: u16,
    pub height: u16,
    pub mip_count: u8,
}

impl TexInfo {
    fn from_header(h: &[u8; 8]) -> Self {
        // 16-bit dims with HIGH/LOW bytes in non-adjacent slots: width =
        // [2]·256+[5], height = [7]·256+[0]; [1]=GTEX format, [4]=mip count. Low
        // byte required for non-256-multiple dims (e.g. 256×64).
        TexInfo {
            format: h[1],
            width: ((h[2] as u16) << 8) | h[5] as u16,
            height: ((h[7] as u16) << 8) | h[0] as u16,
            mip_count: h[4].max(1),
        }
    }

    fn bytes_per_block(self) -> usize {
        if self.format == 24 { 8 } else { 16 } // DXT1 vs DXT3/DXT5
    }

    /// Per-mip `(real, stored)` byte sizes (DXT 4×4 blocks, min 1/mip). `real` =
    /// DXT data the DDS needs; `stored` = what the `.bin` reserves: packs floor
    /// every mip to 16 bytes, so DXT1 8-byte tail mips carry 8 bytes padding.
    fn mips(self) -> impl Iterator<Item = (usize, usize)> {
        let bpb = self.bytes_per_block();
        let (mut w, mut h) = (self.width as usize, self.height as usize);
        (0..self.mip_count).map(move |_| {
            let real = w.div_ceil(4).max(1) * h.div_ceil(4).max(1) * bpb;
            w = (w / 2).max(1);
            h = (h / 2).max(1);
            (real, real.max(16))
        })
    }
}

/// Deobfuscates a `.bin` into a standard DDS -> `(TexInfo, DDS bytes)`. Each
/// mip's real DXT bytes are copied out of the 16-byte-floored stored layout.
pub fn decode_to_dds(name: &str, data: &[u8]) -> ff13_formats::Result<(TexInfo, Vec<u8>)> {
    let (hdr, body) = decode_full(name, data)?;
    let info = TexInfo::from_header(&hdr);
    if !matches!(info.format, 24..=26) || info.width == 0 || info.height == 0 {
        return Err(ff13_formats::FormatError::Unsupported(format!(
            "{name}.bin: unrecognized texture header {hdr:02x?}"
        )));
    }
    let mut dds = dds_header(info);
    let mut pos = 0usize;
    for (real, stored) in info.mips() {
        match body.get(pos..pos + real) {
            Some(slice) => dds.extend_from_slice(slice),
            None => {
                dds.extend_from_slice(body.get(pos..).unwrap_or(&[]));
                break;
            }
        }
        pos += stored;
    }
    Ok((info, dds))
}

fn dds_header(info: TexInfo) -> Vec<u8> {
    let (w, h, mips) = (
        info.width as u32,
        info.height as u32,
        info.mip_count.max(1) as u32,
    );
    let fourcc: &[u8; 4] = match info.format {
        25 => b"DXT3",
        26 => b"DXT5",
        _ => b"DXT1",
    };
    let mut hdr = vec![0u8; 128];
    hdr[0..4].copy_from_slice(b"DDS ");
    let put = |h: &mut [u8], o: usize, v: u32| h[o..o + 4].copy_from_slice(&v.to_le_bytes());
    put(&mut hdr, 4, 124); // dwSize
    let mut flags = 0x1 | 0x2 | 0x4 | 0x1000 | 0x8_0000; // CAPS|HEIGHT|WIDTH|PIXFMT|LINEARSIZE
    if mips > 1 {
        flags |= 0x2_0000; // MIPMAPCOUNT
    }
    put(&mut hdr, 8, flags);
    put(&mut hdr, 12, h);
    put(&mut hdr, 16, w);
    put(
        &mut hdr,
        20,
        w.div_ceil(4).max(1) * h.div_ceil(4).max(1) * info.bytes_per_block() as u32,
    );
    put(&mut hdr, 28, mips);
    put(&mut hdr, 76, 32); // pixelformat dwSize
    put(&mut hdr, 80, 0x4); // DDPF_FOURCC
    hdr[84..88].copy_from_slice(fourcc);
    let mut caps = 0x1000; // TEXTURE
    if mips > 1 {
        caps |= 0x40_0000 | 0x8; // MIPMAP|COMPLEX
    }
    put(&mut hdr, 108, caps);
    hdr
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_known_vector() {
        let mut data = vec![0u8; 8]; // placeholder header
        data.extend_from_slice(&hex("3de94c1179467bfe50b8d28b71bd8309"));
        let plain = decode("c001C_02", &data).unwrap();
        assert_eq!(plain, hex("6108000015151515073aa5317dfdfffa"));
    }

    #[test]
    fn empty_name_is_an_error() {
        assert!(decode("", &[0u8; 16]).is_err());
        assert!(decode_to_dds("", &[0u8; 16]).is_err());
    }

    // Header layout, cross-checked vs the tool's extracted DDS dims.
    #[test]
    fn parses_header_fields() {
        let cases = [
            ("0018040001000004", 24u8, 1024u16, 1024u16, 1u8), // c001C_02 DXT1 1024²
            ("0018080001000004", 24, 2048, 1024, 1),           // C001C_01 DXT1 2048×1024
            ("0018080001000008", 24, 2048, 2048, 1),           // c001N_02 DXT1 2048²
            ("001808000c000008", 24, 2048, 2048, 12),          // C001C_04 DXT1 2048² + mips
            // non-256-multiple: height low byte [0], width low byte [5]
            ("801802000a000000", 24, 512, 128, 10), // f005C_01 DXT1 512×128
            ("4018010001000000", 24, 256, 64, 1),   // n349C_03 DXT1 256×64
            ("801a000008800000", 26, 128, 128, 8),  // f169CM_01 DXT5 128²
        ];
        for (h, fmt, w, ht, mips) in cases {
            let mut hb = [0u8; 8];
            hb.copy_from_slice(&hex(h));
            let info = TexInfo::from_header(&hb);
            assert_eq!(
                (info.format, info.width, info.height, info.mip_count),
                (fmt, w, ht, mips),
                "{h}"
            );
        }
    }

    // Packs floor every mip to 16 bytes; a DXT1 chain's three 8-byte tail mips
    // each carry 8 bytes padding the DDS must skip.
    #[test]
    fn mip_layout_floors_to_16() {
        let info = TexInfo {
            format: 24,
            width: 2048,
            height: 2048,
            mip_count: 12,
        };
        let mips: Vec<_> = info.mips().collect();
        assert_eq!(mips.len(), 12);
        assert_eq!(mips[0], (2097152, 2097152)); // mip0, no floor
        assert_eq!(&mips[9..], &[(8, 16), (8, 16), (8, 16)]); // 4×4, 2×2, 1×1
        let stored: usize = mips.iter().map(|&(_, s)| s).sum();
        let real: usize = mips.iter().map(|&(r, _)| r).sum();
        assert_eq!(stored, 2796240); // matches the .bin body length
        assert_eq!(real, 2796216); // matches the DDS mip-chain length
    }

    // Perturbation at the first body checkpoint must land correctly.
    #[test]
    fn decodes_across_checkpoint() {
        // bytes 5318..5330 straddle the 5323-byte boundary
        let mut data = vec![0u8; 8 + 5318];
        data.extend_from_slice(&hex("ef05c188fdaa69c067c89b86"));
        let plain = decode("c001C_02", &data).unwrap();
        assert_eq!(&plain[5318..5330], &hex("55aa01000000555555aa0100")[..]);
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }
}
