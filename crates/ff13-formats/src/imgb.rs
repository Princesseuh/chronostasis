//! IMGB textures: a `GTEX` header chunk plus pixel data in the paired `.imgb`.

use byteorder::{BigEndian as BE, ByteOrder, LittleEndian as LE};

use crate::wpd::Wpd;
use crate::{FormatError, Result};

const GTEX: &[u8; 4] = b"GTEX";
const DDS_MAGIC: u32 = 0x2053_4444;
const DDS_HEADER_LEN: usize = 128;
const DDPF_FOURCC: u32 = 0x4;

// GTEX texture formats (the `format` byte).
const FMT_ARGB_MIPPED: u8 = 3;
const FMT_ARGB_SINGLE: u8 = 4;
const FMT_DXT1: u8 = 24;
const FMT_DXT3: u8 = 25;
const FMT_DXT5: u8 = 26;

#[derive(Debug, Clone)]
pub struct Gtex {
    /// Offset within the header file, which doubles as a stable identity.
    pub offset: usize,
    pub format: u8,
    pub mip_count: u8,
    /// GTEX byte +8, surfaced raw. Possibly a colour-space flag, but unconfirmed.
    pub unk8: u8,
    pub kind: u8,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    /// In image order.
    pub mips: Vec<(u32, u32)>,
}

impl Gtex {
    fn is_dxt(&self) -> bool {
        matches!(self.format, FMT_DXT1 | FMT_DXT3 | FMT_DXT5)
    }

    /// The DDS body length.
    fn body_len(&self) -> usize {
        self.mips.iter().map(|&(_, s)| s as usize).sum()
    }

    /// Rejects false `GTEX` scan hits by checking the mips fit an `imgb` of `imgb_len` bytes.
    fn is_plausible(&self, imgb_len: usize) -> bool {
        matches!(
            self.format,
            FMT_ARGB_MIPPED | FMT_ARGB_SINGLE | FMT_DXT1 | FMT_DXT3 | FMT_DXT5
        ) && (1..=8192).contains(&self.width)
            && (1..=8192).contains(&self.height)
            && (1..=14).contains(&self.mip_count)
            && !self.mips.is_empty()
            && self.mips.iter().all(|&(s, sz)| {
                (s as usize)
                    .checked_add(sz as usize)
                    .is_some_and(|end| end <= imgb_len)
            })
    }
}

#[derive(Debug, Clone)]
pub struct Texture {
    pub gtex_offset: usize,
    pub width: u16,
    pub height: u16,
    pub format: u8,
    /// See [`Gtex::unk8`].
    pub unk8: u8,
    pub dds: Vec<u8>,
}

pub fn find_gtex(header: &[u8]) -> Vec<usize> {
    let mut offsets = Vec::new();
    if header.len() < 4 {
        return offsets;
    }
    for i in 0..=header.len() - 4 {
        if &header[i..i + 4] == GTEX {
            offsets.push(i);
        }
    }
    offsets
}

pub fn parse_gtex(header: &[u8], offset: usize) -> Result<Gtex> {
    let need = offset + 20;
    if header.len() < need || &header[offset..offset + 4] != GTEX {
        return Err(malformed("no GTEX chunk at offset"));
    }
    let format = header[offset + 6];
    let mip_count = header[offset + 7];
    let unk8 = header[offset + 8];
    let kind = header[offset + 9];
    let width = BE::read_u16(&header[offset + 10..offset + 12]);
    let height = BE::read_u16(&header[offset + 12..offset + 14]);
    let depth = BE::read_u16(&header[offset + 14..offset + 16]);
    let table_off = offset + BE::read_u32(&header[offset + 16..offset + 20]) as usize;

    let count = match kind {
        0 | 4 => mip_count as usize,
        1 | 5 => mip_count as usize * 6,
        2 => 1,
        _ => mip_count as usize,
    };
    let mut mips = Vec::with_capacity(count);
    for i in 0..count {
        let p = table_off + i * 8;
        if header.len() < p + 8 {
            return Err(malformed("mip table out of range"));
        }
        let start = BE::read_u32(&header[p..p + 4]);
        let size = BE::read_u32(&header[p + 4..p + 8]);
        mips.push((start, size));
    }
    Ok(Gtex {
        offset,
        format,
        mip_count,
        unk8,
        kind,
        width,
        height,
        depth,
        mips,
    })
}

/// In file order, which is the index extract and replace both address textures by.
pub fn valid_gtex(header: &[u8], imgb_len: usize) -> Vec<Gtex> {
    find_gtex(header)
        .into_iter()
        .filter_map(|o| parse_gtex(header, o).ok())
        .filter(|g| g.is_plausible(imgb_len))
        .collect()
}

/// Implausible `GTEX` hits are skipped.
pub fn extract(header: &[u8], imgb: &[u8]) -> Result<Vec<Texture>> {
    let mut out = Vec::new();
    for gtex in valid_gtex(header, imgb.len()) {
        let mut dds = dds_header(&gtex)?;
        for &(start, size) in &gtex.mips {
            dds.extend_from_slice(&imgb[start as usize..start as usize + size as usize]);
        }
        out.push(Texture {
            gtex_offset: gtex.offset,
            width: gtex.width,
            height: gtex.height,
            format: gtex.format,
            unk8: gtex.unk8,
            dds,
        });
    }
    Ok(out)
}

/// `(width, height, six face DDS files)`.
pub type CubemapMipLevel = (u16, u16, Vec<Vec<u8>>);

/// Faces come out in DDS cube order (+x -x +y -y +z -z). `None` unless `kind` is 1 or 5; stops
/// before sub-4px mips, which are smaller than a DXT block.
pub fn extract_cubemap_mips(gtex: &Gtex, imgb: &[u8]) -> Option<Vec<CubemapMipLevel>> {
    if !matches!(gtex.kind, 1 | 5) {
        return None;
    }
    let mc = gtex.mip_count.max(1) as usize;
    if gtex.mips.len() < mc * 6 {
        return None;
    }
    let mut levels = Vec::with_capacity(mc);
    for mip in 0..mc {
        let w = (gtex.width >> mip).max(1);
        let h = (gtex.height >> mip).max(1);
        if w < 4 || h < 4 {
            break;
        }
        let mut faces = Vec::with_capacity(6);
        for f in 0..6 {
            // The layout is face-major: each face's mips are contiguous.
            let (start, size) = gtex.mips[f * mc + mip];
            let data = imgb.get(start as usize..start as usize + size as usize)?;
            let face = Gtex {
                mip_count: 1,
                kind: 0,
                width: w,
                height: h,
                mips: vec![(0, size)],
                ..gtex.clone()
            };
            let mut dds = dds_header(&face).ok()?;
            dds.extend_from_slice(data);
            faces.push(dds);
        }
        levels.push((w, h, faces));
    }
    (!levels.is_empty()).then_some(levels)
}

/// Copies only the first texture's pixels, for callers that want a thumbnail plus a count badge.
pub fn first_and_count(header: &[u8], imgb: &[u8]) -> Result<(Texture, usize)> {
    let gtexs = valid_gtex(header, imgb.len());
    let count = gtexs.len();
    let gtex = gtexs
        .into_iter()
        .next()
        .ok_or_else(|| malformed("no valid textures in container"))?;
    let mut dds = dds_header(&gtex)?;
    for &(start, size) in &gtex.mips {
        dds.extend_from_slice(&imgb[start as usize..start as usize + size as usize]);
    }
    Ok((
        Texture {
            gtex_offset: gtex.offset,
            width: gtex.width,
            height: gtex.height,
            format: gtex.format,
            unk8: gtex.unk8,
            dds,
        },
        count,
    ))
}

/// In place, so the DDS must match the original's dimensions, format and mip count.
pub fn replace_in_place(gtex: &Gtex, imgb: &mut [u8], dds: &[u8]) -> Result<()> {
    if dds.len() < DDS_HEADER_LEN || LE::read_u32(&dds[0..4]) != DDS_MAGIC {
        return Err(malformed("not a DDS file"));
    }
    let w = LE::read_u32(&dds[16..20]) as u16;
    let h = LE::read_u32(&dds[12..16]) as u16;
    if w != gtex.width || h != gtex.height {
        return Err(FormatError::Unsupported(format!(
            "DDS is {w}x{h} but the texture is {}x{} (resizing needs a full repack)",
            gtex.width, gtex.height
        )));
    }
    let body = &dds[DDS_HEADER_LEN..];
    if body.len() < gtex.body_len() {
        return Err(malformed(
            "DDS has fewer mip bytes than the texture expects",
        ));
    }
    let mut pos = 0usize;
    for &(start, size) in &gtex.mips {
        let (start, size) = (start as usize, size as usize);
        imgb.get_mut(start..start + size)
            .ok_or_else(|| malformed("mip target out of range in imgb"))?
            .copy_from_slice(&body[pos..pos + size]);
        pos += size;
    }
    Ok(())
}

fn dds_header(gtex: &Gtex) -> Result<Vec<u8>> {
    const DDSD_CAPS: u32 = 0x1;
    const DDSD_HEIGHT: u32 = 0x2;
    const DDSD_WIDTH: u32 = 0x4;
    const DDSD_PITCH: u32 = 0x8;
    const DDSD_PIXELFORMAT: u32 = 0x1000;
    const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;
    const DDSD_LINEARSIZE: u32 = 0x8_0000;
    const DDPF_ALPHAPIXELS: u32 = 0x1;
    const DDPF_RGB: u32 = 0x40;
    const DDSCAPS_COMPLEX: u32 = 0x8;
    const DDSCAPS_TEXTURE: u32 = 0x1000;
    const DDSCAPS_MIPMAP: u32 = 0x40_0000;

    let (w, h, mips) = (
        gtex.width as u32,
        gtex.height as u32,
        gtex.mip_count.max(1) as u32,
    );
    let mut hdr = vec![0u8; DDS_HEADER_LEN];
    LE::write_u32(&mut hdr[0..4], DDS_MAGIC);
    LE::write_u32(&mut hdr[4..8], 124);
    let mut flags = DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PIXELFORMAT;
    if mips > 1 {
        flags |= DDSD_MIPMAPCOUNT;
    }

    let (pf_flags, fourcc, bitcount, linear) = match gtex.format {
        FMT_DXT1 => (
            DDPF_FOURCC,
            u32::from_le_bytes(*b"DXT1"),
            0,
            dxt_size(w, h, 8),
        ),
        FMT_DXT3 => (
            DDPF_FOURCC,
            u32::from_le_bytes(*b"DXT3"),
            0,
            dxt_size(w, h, 16),
        ),
        FMT_DXT5 => (
            DDPF_FOURCC,
            u32::from_le_bytes(*b"DXT5"),
            0,
            dxt_size(w, h, 16),
        ),
        FMT_ARGB_MIPPED | FMT_ARGB_SINGLE => (DDPF_RGB | DDPF_ALPHAPIXELS, 0, 32, w * 4),
        other => return Err(FormatError::Unsupported(format!("GTEX format {other}"))),
    };
    flags |= if gtex.is_dxt() {
        DDSD_LINEARSIZE
    } else {
        DDSD_PITCH
    };

    LE::write_u32(&mut hdr[8..12], flags);
    LE::write_u32(&mut hdr[12..16], h);
    LE::write_u32(&mut hdr[16..20], w);
    LE::write_u32(&mut hdr[20..24], linear);
    LE::write_u32(&mut hdr[28..32], mips);
    LE::write_u32(&mut hdr[76..80], 32);
    LE::write_u32(&mut hdr[80..84], pf_flags);
    LE::write_u32(&mut hdr[84..88], fourcc);
    LE::write_u32(&mut hdr[88..92], bitcount);
    if !gtex.is_dxt() {
        LE::write_u32(&mut hdr[92..96], 0x00FF_0000);
        LE::write_u32(&mut hdr[96..100], 0x0000_FF00);
        LE::write_u32(&mut hdr[100..104], 0x0000_00FF);
        LE::write_u32(&mut hdr[104..108], 0xFF00_0000);
    }
    let mut caps = DDSCAPS_TEXTURE;
    if mips > 1 {
        caps |= DDSCAPS_COMPLEX | DDSCAPS_MIPMAP;
    }
    LE::write_u32(&mut hdr[108..112], caps);
    Ok(hdr)
}

fn dxt_size(w: u32, h: u32, block_bytes: u32) -> u32 {
    w.div_ceil(4).max(1) * h.div_ceil(4).max(1) * block_bytes
}

/// Rounds DXT dimensions up to a multiple of 4, mutating `w` and `h`.
fn compute_mip_size(format: u8, w: &mut u16, h: &mut u16) -> u32 {
    match format {
        FMT_ARGB_MIPPED | FMT_ARGB_SINGLE => *h as u32 * *w as u32 * 4,
        FMT_DXT1 => {
            *h += (4 - *h % 4) % 4;
            *w += (4 - *w % 4) % 4;
            *h as u32 * *w as u32 / 2
        }
        FMT_DXT3 | FMT_DXT5 => {
            *h += (4 - *h % 4) % 4;
            *w += (4 - *w % 4) % 4;
            *h as u32 * *w as u32
        }
        _ => 0,
    }
}

/// For block formats the height floors at 4 but the width does not.
fn next_mip(format: u8, w: &mut u16, h: &mut u16) {
    if format == FMT_ARGB_MIPPED {
        *h = if *h == 1 { 1 } else { *h / 2 };
        *w = if *w == 1 { 1 } else { *w / 2 };
    } else {
        *h /= 2;
        if *h < 4 {
            *h = 4;
        }
        *w /= 2;
    }
}

/// Classic 2D only, but the DDS may have any dimensions and mip count.
pub fn repack_type2(block: &[u8], dds: &[u8], imgb_out: &mut Vec<u8>) -> Result<Vec<u8>> {
    if dds.len() < DDS_HEADER_LEN || LE::read_u32(&dds[0..4]) != DDS_MAGIC {
        return Err(malformed("not a DDS file"));
    }
    let h = LE::read_u32(&dds[12..16]) as u16;
    let w = LE::read_u32(&dds[16..20]) as u16;
    let mip_count = (LE::read_u32(&dds[28..32]).max(1)) as u8;
    let format = match &dds[84..88] {
        b"DXT1" => FMT_DXT1,
        b"DXT3" => FMT_DXT3,
        b"DXT5" => FMT_DXT5,
        fourcc if LE::read_u32(&dds[80..84]) & DDPF_FOURCC != 0 => {
            return Err(FormatError::Unsupported(format!(
                "DDS fourcc {:?} (only DXT1/3/5 or uncompressed ARGB can be repacked)",
                String::from_utf8_lossy(fourcc)
            )));
        }
        _ if mip_count > 1 => FMT_ARGB_MIPPED,
        _ => FMT_ARGB_SINGLE,
    };

    let gtex_off = *find_gtex(block)
        .first()
        .ok_or_else(|| malformed("no GTEX in header block"))?;
    let table_at = gtex_off + 24;
    if block.len() < table_at {
        return Err(malformed("header block too small"));
    }

    let mut nb = block[..table_at].to_vec();
    nb.resize(table_at + mip_count as usize * 8, 0);
    while !nb.len().is_multiple_of(16) {
        nb.push(0);
    }
    nb[gtex_off + 6] = format;
    nb[gtex_off + 7] = mip_count;
    nb[gtex_off + 9] = 0;
    nb[gtex_off + 10..gtex_off + 12].copy_from_slice(&w.to_be_bytes());
    nb[gtex_off + 12..gtex_off + 14].copy_from_slice(&h.to_be_bytes());
    nb[gtex_off + 14..gtex_off + 16].copy_from_slice(&1u16.to_be_bytes());
    nb[gtex_off + 16..gtex_off + 20].copy_from_slice(&24u32.to_be_bytes());

    let body = &dds[DDS_HEADER_LEN..];
    let (mut cw, mut ch) = (w, h);
    let mut body_pos = 0usize;
    let imgb_start = imgb_out.len();
    for i in 0..mip_count as usize {
        let size = compute_mip_size(format, &mut cw, &mut ch) as usize;
        let start = imgb_out.len() as u32;
        let t = table_at + i * 8;
        nb[t..t + 4].copy_from_slice(&start.to_be_bytes());
        nb[t + 4..t + 8].copy_from_slice(&(size as u32).to_be_bytes());
        let chunk = body
            .get(body_pos..body_pos + size)
            .ok_or_else(|| malformed("DDS has fewer mip bytes than expected"))?;
        imgb_out.extend_from_slice(chunk);
        body_pos += size;
        if size < 16 {
            imgb_out.resize(imgb_out.len() + (16 - size), 0);
        }
        next_mip(format, &mut cw, &mut ch);
    }
    // A wrong size field here loads the wrong byte count and draws white.
    let imgb_size = imgb_out.len() - imgb_start;
    let orig_w = BE::read_u16(&block[gtex_off + 10..gtex_off + 12]);
    let upscaled = w > orig_w;
    let total = if upscaled {
        imgb_size as u32
    } else {
        (imgb_size + table_at + mip_count as usize * 8) as u32
    };
    nb[16..20].copy_from_slice(&total.to_le_bytes());
    Ok(nb)
}

/// Copies pixels verbatim and preserves the `0x10` size field; re-encoding from DDS instead drifts
/// the padding and size, which breaks zone lighting.
pub fn repack_2d_passthrough(
    block: &[u8],
    original_imgb: &[u8],
    imgb_out: &mut Vec<u8>,
) -> Result<Vec<u8>> {
    let goff = *find_gtex(block)
        .first()
        .ok_or_else(|| malformed("no GTEX"))?;
    let g = parse_gtex(block, goff)?;
    if g.mips.is_empty() {
        return Ok(block.to_vec());
    }
    let table_at = goff + 24;
    let min_s = g.mips.iter().map(|&(s, _)| s as usize).min().unwrap();
    let max_e = g
        .mips
        .iter()
        .map(|&(s, sz)| s as usize + (sz as usize).max(16))
        .max()
        .unwrap()
        .min(original_imgb.len());
    let span = original_imgb
        .get(min_s..max_e)
        .ok_or_else(|| malformed("texture imgb span out of range"))?;
    let new_start = imgb_out.len();
    imgb_out.extend_from_slice(span);

    let mut nb = block[..table_at].to_vec();
    nb.resize(table_at + g.mips.len() * 8, 0);
    while !nb.len().is_multiple_of(16) {
        nb.push(0);
    }
    nb[goff + 14..goff + 16].copy_from_slice(&1u16.to_be_bytes());
    nb[goff + 16..goff + 20].copy_from_slice(&24u32.to_be_bytes());
    for (k, &(orig_off, sz)) in g.mips.iter().enumerate() {
        let new_off = (new_start + (orig_off as usize - min_s)) as u32;
        let p = table_at + k * 8;
        nb[p..p + 4].copy_from_slice(&new_off.to_be_bytes());
        nb[p + 4..p + 8].copy_from_slice(&sz.to_be_bytes());
    }
    Ok(nb)
}

/// For cubemaps and volumes, which cannot be mip-substituted. Only the mip offsets are repointed;
/// forcing the descriptor to 2D corrupts the texture and blows out zone lighting.
pub fn copy_texture_verbatim(
    block: &[u8],
    original_imgb: &[u8],
    imgb_out: &mut Vec<u8>,
) -> Result<Vec<u8>> {
    let goff = *find_gtex(block)
        .first()
        .ok_or_else(|| malformed("no GTEX"))?;
    let g = parse_gtex(block, goff)?;
    if g.mips.is_empty() {
        return Ok(block.to_vec());
    }
    let min_s = g.mips.iter().map(|&(s, _)| s as usize).min().unwrap();
    // The last mip's 16-byte pad can run past the imgb end, so clamp to the real data.
    let max_e = g
        .mips
        .iter()
        .map(|&(s, sz)| s as usize + (sz as usize).max(16))
        .max()
        .unwrap()
        .min(original_imgb.len());
    let span = original_imgb
        .get(min_s..max_e)
        .ok_or_else(|| malformed("texture imgb span out of range"))?;
    let new_min = imgb_out.len();
    imgb_out.extend_from_slice(span);

    let mut nb = block.to_vec();
    let table_off = goff + BE::read_u32(&block[goff + 16..goff + 20]) as usize;
    let delta = new_min as i64 - min_s as i64;
    for k in 0..g.mips.len() {
        let p = table_off + k * 8;
        let old = BE::read_u32(&nb[p..p + 4]) as i64;
        nb[p..p + 4].copy_from_slice(&((old + delta) as u32).to_be_bytes());
    }
    Ok(nb)
}

/// The members holding a valid `GTEX` against `imgb`.
pub fn xgr_texture_members(xgr: &[u8], imgb: &[u8]) -> Result<Vec<String>> {
    let wpd = Wpd::parse(xgr)?;
    Ok(wpd
        .entries
        .iter()
        .filter(|e| !valid_gtex(&e.data, imgb.len()).is_empty())
        .map(|e| e.file_name())
        .collect())
}

/// `replacements` is keyed by member file-name, e.g. `monster.txbh`. Members it does not name
/// round-trip their pixels into the fresh imgb. Single-GTEX members only.
pub fn repack_xgr(
    xgr: &[u8],
    imgb: &[u8],
    replacements: &std::collections::HashMap<String, Vec<u8>>,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut wpd = Wpd::parse(xgr)?;
    let mut new_imgb = Vec::new();
    let mut used = 0usize;
    for e in wpd.entries.iter_mut() {
        let gtex = valid_gtex(&e.data, imgb.len());
        if gtex.is_empty() {
            continue;
        }
        if gtex.len() > 1 {
            return Err(FormatError::Unsupported(format!(
                "member '{}' holds {} textures in one block (multi-GTEX repack unsupported)",
                e.file_name(),
                gtex.len()
            )));
        }
        let dds = match replacements.get(&e.file_name()) {
            Some(dds) => {
                used += 1;
                dds.clone()
            }
            None => {
                extract(&e.data, imgb)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| malformed("texture member failed to round-trip"))?
                    .dds
            }
        };
        let new_block = repack_type2(&e.data, &dds, &mut new_imgb)?;
        e.data = new_block;
    }
    if used != replacements.len() {
        return Err(FormatError::Unsupported(format!(
            "{} replacement(s) matched no texture member",
            replacements.len() - used
        )));
    }
    Ok((wpd.write(), new_imgb))
}

fn malformed(detail: &str) -> FormatError {
    FormatError::Malformed {
        format: "IMGB",
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_and_replace_dxt1() {
        let (w, h) = (8u16, 8u16);
        let mip_size = dxt_size(w as u32, h as u32, 8);
        let imgb: Vec<u8> = (0..mip_size as usize).map(|i| i as u8).collect();

        let mut header = vec![0u8; 28];
        header[0..4].copy_from_slice(GTEX);
        header[6] = FMT_DXT1;
        header[7] = 1;
        header[9] = 0;
        BE::write_u16(&mut header[10..12], w);
        BE::write_u16(&mut header[12..14], h);
        BE::write_u32(&mut header[16..20], 20);
        BE::write_u32(&mut header[20..24], 0);
        BE::write_u32(&mut header[24..28], mip_size);

        let textures = extract(&header, &imgb).unwrap();
        assert_eq!(textures.len(), 1);
        let dds = &textures[0].dds;
        assert_eq!(LE::read_u32(&dds[0..4]), DDS_MAGIC);
        assert_eq!(LE::read_u32(&dds[16..20]), w as u32);
        assert_eq!(dds.len(), DDS_HEADER_LEN + mip_size as usize);
        assert_eq!(&dds[DDS_HEADER_LEN..], &imgb[..]);

        let mut edited = dds.clone();
        for b in &mut edited[DDS_HEADER_LEN..] {
            *b = 0xAB;
        }
        let gtex = parse_gtex(&header, 0).unwrap();
        let mut imgb2 = imgb.clone();
        replace_in_place(&gtex, &mut imgb2, &edited).unwrap();
        assert!(imgb2.iter().all(|&b| b == 0xAB));

        let mut wrong = edited.clone();
        LE::write_u32(&mut wrong[16..20], 16);
        assert!(replace_in_place(&gtex, &mut imgb2, &wrong).is_err());
    }

    #[test]
    fn parse_gtex_fields_and_plausibility() {
        let mut hdr = vec![0u8; 20 + 2 * 8];
        hdr[0..4].copy_from_slice(GTEX);
        hdr[6] = FMT_DXT1;
        hdr[7] = 2;
        hdr[8] = 0x01;
        hdr[9] = 0;
        hdr[10..12].copy_from_slice(&64u16.to_be_bytes());
        hdr[12..14].copy_from_slice(&64u16.to_be_bytes());
        hdr[14..16].copy_from_slice(&1u16.to_be_bytes());
        hdr[16..20].copy_from_slice(&20u32.to_be_bytes());
        hdr[20..24].copy_from_slice(&0u32.to_be_bytes());
        hdr[24..28].copy_from_slice(&2048u32.to_be_bytes());
        hdr[28..32].copy_from_slice(&2048u32.to_be_bytes());
        hdr[32..36].copy_from_slice(&512u32.to_be_bytes());

        let g = parse_gtex(&hdr, 0).unwrap();
        assert_eq!((g.format, g.mip_count, g.kind), (FMT_DXT1, 2, 0));
        assert_eq!(g.unk8, 0x01, "GTEX +8 read into unk8");
        assert_eq!((g.width, g.height, g.depth), (64, 64, 1));
        assert_eq!(g.mips, vec![(0, 2048), (2048, 512)]);

        assert_eq!(valid_gtex(&hdr, 2560).len(), 1, "fits → accepted");
        assert!(
            valid_gtex(&hdr, 100).is_empty(),
            "mips exceed imgb → filtered"
        );
    }

    /// A cubemap (kind 1) expands to mip_count × 6 mip entries.
    #[test]
    fn cubemap_expands_to_six_faces() {
        let mut hdr = vec![0u8; 20 + 6 * 8];
        hdr[0..4].copy_from_slice(GTEX);
        hdr[6] = FMT_DXT1;
        hdr[7] = 1;
        hdr[9] = 1;
        hdr[10..12].copy_from_slice(&16u16.to_be_bytes());
        hdr[12..14].copy_from_slice(&16u16.to_be_bytes());
        hdr[16..20].copy_from_slice(&20u32.to_be_bytes());
        for i in 0..6 {
            let p = 20 + i * 8;
            hdr[p..p + 4].copy_from_slice(&((i as u32) * 128).to_be_bytes());
            hdr[p + 4..p + 8].copy_from_slice(&128u32.to_be_bytes());
        }
        let g = parse_gtex(&hdr, 0).unwrap();
        assert_eq!(g.kind, 1);
        assert_eq!(g.mips.len(), 6, "cubemap = 6 faces");
    }
}
