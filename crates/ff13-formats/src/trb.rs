//! TRB (`SEDBRES`) resource bundle, the texture-header container paired with an `.imgb`.

use std::collections::HashMap;

use byteorder::{BigEndian as BE, ByteOrder, LittleEndian as LE};

use crate::{FormatError, Result, imgb};

const MAGIC: &[u8; 8] = b"SEDBRES ";
const HEADER_LEN: usize = 64;

struct Resource {
    index: u32,
    type_val: u32,
    offset: u32,
    size: u32,
    data: Vec<u8>,
}

/// A `RESOURCE_ID` table parsed into per-resource `(short_name_slot, full_name)`.
pub type RidNames = (Vec<[u8; 16]>, Vec<Vec<u8>>);

pub struct Trb {
    header: [u8; HEADER_LEN],
    ids_start_offset: u32,
    rid_resource: usize,
    rid_orig_offset: u32,
    resources: Vec<Resource>,
    /// Kept verbatim, so the writers can reproduce the original padding and overlapping resources.
    region: Vec<u8>,
}

impl Trb {
    pub fn parse(trb: &[u8]) -> Result<Trb> {
        if trb.len() < HEADER_LEN || &trb[0..8] != MAGIC {
            return Err(FormatError::BadMagic {
                expected: "SEDBRES ".into(),
                found: format!("{:?}", String::from_utf8_lossy(&trb[0..trb.len().min(8)])),
            });
        }
        let ids_start_offset = LE::read_u32(&trb[52..56]);
        let resource_count = LE::read_u32(&trb[56..60]) as usize;
        let data_start = resource_count
            .checked_mul(16)
            .and_then(|n| n.checked_add(HEADER_LEN))
            .filter(|&n| n <= trb.len())
            .ok_or_else(|| malformed("resource table out of range"))?;

        let mut resources: Vec<Resource> = Vec::with_capacity(resource_count);
        let mut rid_resource = 0;
        let mut rid_orig_offset = 0;
        for i in 0..resource_count {
            let e = HEADER_LEN + i * 16;
            let index = LE::read_u32(&trb[e..]);
            let offset = LE::read_u32(&trb[e + 4..]);
            let size = LE::read_u32(&trb[e + 8..]);
            let type_val = LE::read_u32(&trb[e + 12..]);
            // The span math in the writers relies on ascending offsets.
            if resources.last().is_some_and(|p| offset < p.offset) {
                return Err(malformed("resource offsets not ascending"));
            }
            let start = data_start + offset as usize;
            // The final resource's declared size can run past EOF, where padding was dropped.
            let end = (start + size as usize).min(trb.len());
            let data = trb
                .get(start..end)
                .ok_or_else(|| malformed("resource data out of range"))?
                .to_vec();
            if ids_start_offset >= offset && ids_start_offset - offset < size {
                rid_resource = i;
                rid_orig_offset = offset;
            }
            resources.push(Resource {
                index,
                type_val,
                offset,
                size,
                data,
            });
        }
        let mut header = [0u8; HEADER_LEN];
        header.copy_from_slice(&trb[..HEADER_LEN]);
        let region = trb[data_start..].to_vec();
        Ok(Trb {
            header,
            ids_start_offset,
            rid_resource,
            rid_orig_offset,
            resources,
            region,
        })
    }

    fn is_texture(data: &[u8]) -> bool {
        !imgb::find_gtex(data).is_empty()
    }

    pub fn resource_count(&self) -> usize {
        self.resources.len()
    }

    pub fn resource_data(&self, i: usize) -> Option<&[u8]> {
        self.resources.get(i).map(|r| r.data.as_slice())
    }

    /// 2 = generic, 0 = `RESOURCE_ID`/`RESOURCE_TYPE`.
    pub fn resource_type(&self, i: usize) -> Option<u32> {
        self.resources.get(i).map(|r| r.type_val)
    }

    /// Characters carry a `SEDBSKL` resource; most props and background do not.
    pub fn skeleton(&self) -> Option<crate::skl::Skeleton> {
        self.resources
            .iter()
            .filter(|r| r.data.starts_with(b"SEDBSKL"))
            .find_map(|r| crate::skl::Skeleton::parse(&r.data))
    }

    pub fn sockets(&self) -> Option<crate::elb::Sockets> {
        self.resources
            .iter()
            .filter(|r| r.data.starts_with(b"SEDBelb"))
            .find_map(|r| crate::elb::Sockets::parse(&r.data))
    }

    /// Absolute range in the serialized file, for in-place length-preserving patches.
    pub fn resource_abs_span(&self, i: usize) -> Option<(usize, usize)> {
        let r = self.resources.get(i)?;
        let start = HEADER_LEN + self.resources.len() * 16 + r.offset as usize;
        Some((start, start + r.size as usize))
    }

    pub fn header(&self) -> &[u8; HEADER_LEN] {
        &self.header
    }

    /// Matches on the resource data's leading tag, e.g. `SEDBwrb`.
    pub fn find_resource(&self, tag: &[u8]) -> Option<usize> {
        self.resources.iter().position(|r| r.data.starts_with(tag))
    }

    /// Recomputes offsets and `IDsStartOffset`, leaving the header, indices, types and `.imgb` alone.
    pub fn serialize_replacing(&self, idx: usize, new_data: &[u8]) -> Result<Vec<u8>> {
        let n = self.resources.len();
        let old_off = self.resources[idx].offset as usize;
        if let Some(next) = self.resources.get(idx + 1) {
            // Shrinking past the overlap the next resource relies on would place it inside `new_data`.
            let span = (next.offset - self.resources[idx].offset) as usize;
            if span + new_data.len() < self.resources[idx].size as usize {
                return Err(malformed(
                    "replacement smaller than the overlapped resource span",
                ));
            }
        }
        let mut offsets = vec![0u32; n];
        for (i, r) in self.resources.iter().enumerate().take(idx) {
            offsets[i] = r.offset;
        }
        offsets[idx] = old_off as u32;

        let mut region = self.region[..old_off.min(self.region.len())].to_vec();
        let place = |region: &mut Vec<u8>, off: usize, data: &[u8]| {
            if region.len() < off + data.len() {
                region.resize(off + data.len(), 0);
            }
            region[off..off + data.len()].copy_from_slice(data);
        };
        place(&mut region, old_off, new_data);
        // Preserving each input span, rather than repacking, is what keeps the original spacing.
        let footprint = |j: usize| -> usize {
            if j + 1 >= n {
                return self.resources[j].data.len();
            }
            let span = (self.resources[j + 1].offset - self.resources[j].offset) as usize;
            if j == idx {
                span + new_data.len() - self.resources[idx].size as usize
            } else {
                span
            }
        };
        let mut running = old_off + footprint(idx);
        for j in idx + 1..n {
            let data = &self.resources[j].data;
            let off = if j == self.rid_resource && j > 0 {
                // RID overlaps the previous resource by their longest suffix/prefix match, unrounded.
                let prev = &self.resources[j - 1].data;
                let prev_off = offsets[j - 1] as usize;
                let max = prev.len().min(data.len());
                let mut k = 0;
                for c in (1..=max).rev() {
                    if prev[prev.len() - c..] == data[..c] {
                        k = c;
                        break;
                    }
                }
                prev_off + (prev.len() - k)
            } else {
                running
            };
            offsets[j] = off as u32;
            place(&mut region, off, data);
            running = off + footprint(j);
        }
        let orig_end = self
            .resources
            .iter()
            .map(|r| r.offset as usize + r.size as usize)
            .max()
            .unwrap_or(0);
        if orig_end < self.region.len() {
            region.extend_from_slice(&self.region[orig_end..]);
        }

        // The pool can spill past the RID's declared size, so renumber over the whole tail.
        let ids_start = offsets[self.rid_resource] + (self.ids_start_offset - self.rid_orig_offset);
        let mut fxx = 0u32;
        let mut i = ids_start as usize;
        while i < region.len() {
            if is_fxx_prefix(&region[i..]) {
                region[i..i + 3].copy_from_slice(format!("F{fxx:02X}").as_bytes());
                fxx += 1;
            }
            while i < region.len() && region[i] != 0 {
                i += 1;
            }
            i += 1;
        }

        let mut out = Vec::with_capacity(HEADER_LEN + n * 16 + region.len());
        out.extend_from_slice(&self.header);
        for (i, r) in self.resources.iter().enumerate() {
            let size = if i == idx {
                new_data.len() as u32
            } else {
                r.size
            };
            out.extend_from_slice(&r.index.to_le_bytes());
            out.extend_from_slice(&offsets[i].to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&r.type_val.to_le_bytes());
        }
        out.extend_from_slice(&region);
        let total = out.len() as u32;
        out[16..20].copy_from_slice(&total.to_le_bytes());
        out[52..56].copy_from_slice(&ids_start.to_le_bytes());
        Ok(out)
    }

    pub fn rid_resource(&self) -> usize {
        self.rid_resource
    }

    /// Shorts are fixed 16-byte NUL-padded slots; fulls are the NUL-terminated pool at `IDsStartOffset`.
    pub fn rid_names(&self) -> Option<RidNames> {
        let rid = &self.resources.get(self.rid_resource)?.data;
        let n = self.resources.len();
        let mut shorts = Vec::with_capacity(n);
        for i in 0..n {
            let s = rid.get(i * 16..i * 16 + 16)?;
            shorts.push(<[u8; 16]>::try_from(s).ok()?);
        }
        // The pool often spills past the RID into following resources, so read the whole region.
        let mut fulls = Vec::with_capacity(n);
        let mut cur = Vec::new();
        for &b in self.region.get(self.ids_start_offset as usize..)? {
            if b == 0 {
                fulls.push(std::mem::take(&mut cur));
                if fulls.len() == n {
                    break;
                }
            } else {
                cur.push(b);
            }
        }
        // Some models leave trailing resources unnamed, so pad to keep the vec indexable by resource.
        fulls.resize(n, Vec::new());
        Some((shorts, fulls))
    }

    /// Geometry-correct but not a finished model: material and texture-binding resources still need
    /// index remapping.
    #[cfg(test)]
    pub(crate) fn combine_resources(
        sources: &[&Trb],
        map: &[(usize, usize)],
    ) -> Option<Vec<Vec<u8>>> {
        map.iter()
            .map(|&(m, idx)| sources.get(m)?.resource_data(idx).map(<[u8]>::to_vec))
            .collect()
    }

    /// Reproduces the load-time outfit-swap combine. `renames` are `(old, new)` byte pairs
    /// retokenised through `SEDBshd` and the `RESOURCE_ID` shorts, e.g. `c_c006` -> `c_c206`.
    pub fn combine_model(
        out_base: &Trb,
        sources: &[(&Trb, &[u8])],
        map: &[(usize, usize)],
        renames: &[(Vec<u8>, Vec<u8>)],
    ) -> Option<(Vec<u8>, Vec<u8>)> {
        let mut data: Vec<Vec<u8>> = Vec::with_capacity(map.len());
        let mut types: Vec<u32> = Vec::with_capacity(map.len());
        for &(m, idx) in map {
            let (t, _) = *sources.get(m)?;
            data.push(t.resource_data(idx)?.to_vec());
            types.push(t.resource_type(idx)?);
        }

        let imgbs: Vec<&[u8]> = sources.iter().map(|&(_, i)| i).collect();
        // Block starts, so reassemble can copy each texture's full slot: the smallest mip's BC-block
        // storage runs past its declared size.
        let src_blocks: Vec<Vec<usize>> = sources
            .iter()
            .map(|&(t, imgb)| {
                let mut starts: Vec<usize> = (0..t.resource_count())
                    .filter_map(|i| {
                        let d = t.resource_data(i)?;
                        let g = imgb::parse_gtex(d, *imgb::find_gtex(d).first()?).ok()?;
                        g.mips.iter().map(|&(s, _)| s as usize).min()
                    })
                    .collect();
                starts.push(imgb.len());
                starts.sort_unstable();
                starts
            })
            .collect();
        let new_imgb = Self::reassemble_imgb(&mut data, map, &imgbs, &src_blocks)?;

        let apply = |buf: &[u8]| -> Vec<u8> {
            renames.iter().fold(buf.to_vec(), |acc, (old, new)| {
                replace_bytes(&acc, old, new)
            })
        };
        for d in data.iter_mut() {
            if d.starts_with(b"SEDBshd") {
                *d = apply(d);
            }
        }

        let rid_pos = map
            .iter()
            .position(|&(m, idx)| idx == sources[m].0.rid_resource())?;
        let src_rid: Vec<RidNames> = sources
            .iter()
            .map(|&(t, _)| t.rid_names())
            .collect::<Option<_>>()?;
        let mut short_slots: Vec<[u8; 16]> = Vec::with_capacity(map.len());
        let mut full_names: Vec<Vec<u8>> = Vec::with_capacity(map.len());
        let mut fxx = 0u32;
        for &(m, idx) in map {
            let (s, f) = &src_rid[m];
            let renamed = apply(&s[idx]);
            let mut slot = [0u8; 16];
            let k = renamed.len().min(16);
            slot[..k].copy_from_slice(&renamed[..k]);
            short_slots.push(slot);
            let mut name = f[idx].clone();
            if is_fxx_prefix(&name) {
                name[..3].copy_from_slice(format!("F{fxx:02X}").as_bytes());
                fxx += 1;
            } else {
                // Rig names follow the output model rather than the donor, so skip the `puls` rename.
                name = renames
                    .iter()
                    .filter(|(old, _)| !old.windows(4).any(|w| w == b"puls"))
                    .fold(name, |acc, (old, new)| replace_bytes(&acc, old, new));
            }
            full_names.push(name);
        }

        let pool_start = short_slots.len() * 16;
        let mut rid: Vec<u8> = short_slots.iter().flatten().copied().collect();
        for n in &full_names {
            rid.extend_from_slice(n);
            rid.push(0);
        }
        data[rid_pos] = rid;

        if let Some(bxt_pos) = data.iter().position(|d| d.starts_with(b"bxt\0")) {
            let model_bxt = |t: &Trb| {
                (0..t.resource_count()).find_map(|i| {
                    t.resource_data(i)
                        .filter(|d| d.starts_with(b"bxt\0"))
                        .map(<[u8]>::to_vec)
                })
            };
            if let Some(target_bxt) = model_bxt(out_base) {
                let donor_bxts: Vec<Vec<u8>> = sources
                    .iter()
                    .filter(|&&(t, _)| !std::ptr::eq(t, out_base))
                    .filter_map(|&(t, _)| model_bxt(t))
                    .collect();
                let donor_refs: Vec<&[u8]> = donor_bxts.iter().map(Vec::as_slice).collect();
                let phb_count = data.iter().filter(|d| d.starts_with(b"SEDBPHB")).count();
                data[bxt_pos] = build_bxt(
                    &target_bxt,
                    &donor_refs,
                    &short_slots,
                    &full_names,
                    out_base.resource_count(),
                    phb_count,
                );
            }
        }

        // Footprint = min(source span, round_up_16(size)), the combine-writer's own rule.
        let mut region = Vec::new();
        let mut offsets = Vec::with_capacity(data.len());
        for (i, d) in data.iter().enumerate() {
            offsets.push(region.len() as u32);
            region.extend_from_slice(d);
            if i + 1 == data.len() {
                continue;
            }
            let (m, r) = map[i];
            let src = &sources[m].0;
            let slot = if r + 1 < src.resources.len() {
                let span = (src.resources[r + 1].offset - src.resources[r].offset) as usize;
                span.min(d.len().next_multiple_of(16))
            } else {
                src.region
                    .len()
                    .saturating_sub(src.resources[r].offset as usize)
            };
            region.resize(region.len() + slot.saturating_sub(d.len()), 0);
        }
        let ids_start = offsets[rid_pos] + pool_start as u32;

        let count = data.len() as u32;
        let mut out = Vec::with_capacity(HEADER_LEN + data.len() * 16 + region.len());
        out.extend_from_slice(out_base.header());
        // The declared size covers the two tables and header only; the name pool spills past it.
        let rid_size = 32 * count + 64;
        for (i, d) in data.iter().enumerate() {
            let size = if i == rid_pos {
                rid_size
            } else {
                d.len() as u32
            };
            out.extend_from_slice(&(i as u32).to_le_bytes());
            out.extend_from_slice(&offsets[i].to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&types[i].to_le_bytes());
        }
        out.extend_from_slice(&region);
        let total = out.len() as u32;
        out[16..20].copy_from_slice(&total.to_le_bytes());
        out[48..52].copy_from_slice(&count.to_le_bytes());
        out[52..56].copy_from_slice(&ids_start.to_le_bytes());
        out[56..60].copy_from_slice(&count.to_le_bytes());
        Some((out, new_imgb))
    }

    /// Relocates each texture's mip block into a fresh imgb and rewrites the `GTEX` mip offsets in
    /// `resources` to match, mutating them in place.
    pub fn reassemble_imgb(
        resources: &mut [Vec<u8>],
        map: &[(usize, usize)],
        source_imgbs: &[&[u8]],
        src_block_starts: &[Vec<usize>],
    ) -> Option<Vec<u8>> {
        let mut new_imgb: Vec<u8> = Vec::new();
        for (data, &(m, _)) in resources.iter_mut().zip(map) {
            let src_imgb = *source_imgbs.get(m)?;
            let Some(&goff) = imgb::find_gtex(data).first() else {
                continue;
            };
            let Ok(g) = imgb::parse_gtex(data, goff) else {
                continue;
            };
            if g.mips.is_empty() {
                continue;
            }
            let block_start = g.mips.iter().map(|&(s, _)| s).min()? as usize;
            let block_end = g.mips.iter().map(|&(s, sz)| s + sz).max()? as usize;
            if block_end > src_imgb.len() {
                continue;
            }
            while !new_imgb.len().is_multiple_of(16) {
                new_imgb.push(0);
            }
            let new_start = new_imgb.len();
            // Copy up to the next texture's block start, not the declared size: the smallest mip's
            // BC-block storage runs past it, and the last slot's tail is uninitialised heap.
            let copy_end = src_block_starts
                .get(m)
                .and_then(|b| b.iter().copied().find(|&s| s > block_start))
                .unwrap_or_else(|| block_end.next_multiple_of(16))
                .min(src_imgb.len());
            new_imgb.extend_from_slice(src_imgb.get(block_start..copy_end)?);
            let delta = new_start as i64 - block_start as i64;

            let table_off = goff + BE::read_u32(&data[goff + 16..goff + 20]) as usize;
            for i in 0..g.mips.len() {
                let p = table_off + i * 8;
                let nw = (BE::read_u32(&data[p..p + 4]) as i64 + delta) as u32;
                data[p..p + 4].copy_from_slice(&nw.to_be_bytes());
            }
        }
        while !new_imgb.len().is_multiple_of(16) {
            new_imgb.push(0);
        }
        Some(new_imgb)
    }

    pub fn texture_resources(&self) -> Vec<usize> {
        (0..self.resources.len())
            .filter(|&i| Self::is_texture(&self.resources[i].data))
            .collect()
    }

    /// One name per resource in order, e.g. `F03\c001C_02.win32`; empty when the table is unlocatable.
    pub fn resource_names(&self) -> Vec<String> {
        let Some(rid) = self.resources.get(self.rid_resource).map(|r| &r.data) else {
            return Vec::new();
        };
        let start = self.ids_start_offset.saturating_sub(self.rid_orig_offset) as usize;
        let mut names = Vec::with_capacity(self.resources.len());
        let mut cur = Vec::new();
        for &b in rid.get(start..).unwrap_or(&[]) {
            if b == 0 {
                names.push(String::from_utf8_lossy(&cur).into_owned());
                cur.clear();
                if names.len() == self.resources.len() {
                    break;
                }
            } else {
                cur.push(b);
            }
        }
        names
    }

    /// Returns `(resource_index, dds_bytes)` per texture resource.
    pub fn extract_textures(&self, imgb_data: &[u8]) -> Result<Vec<(usize, Vec<u8>)>> {
        let mut out = Vec::new();
        for i in self.texture_resources() {
            if let Some(t) = imgb::extract(&self.resources[i].data, imgb_data)?
                .into_iter()
                .next()
            {
                out.push((i, t.dds));
            }
        }
        Ok(out)
    }

    /// Only the 2D textures listed in `to_encode` are re-encoded; the rest pass through byte-exact,
    /// because re-encoding drifts the mip padding and size field and breaks lighting-baked zone textures.
    pub fn repack(
        &self,
        original_imgb: &[u8],
        to_encode: &HashMap<usize, Vec<u8>>,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let mut new_imgb = Vec::new();
        let mut new_data: Vec<Vec<u8>> = Vec::with_capacity(self.resources.len());
        let first_gtex: Vec<Option<usize>> = self
            .resources
            .iter()
            .map(|r| imgb::find_gtex(&r.data).first().copied())
            .collect();
        for (i, res) in self.resources.iter().enumerate() {
            if let Some(goff) = first_gtex[i] {
                let is_2d = matches!(res.data.get(goff + 9), Some(0) | Some(4));
                if !is_2d {
                    new_data.push(imgb::copy_texture_verbatim(
                        &res.data,
                        original_imgb,
                        &mut new_imgb,
                    )?);
                } else if let Some(dds) = to_encode.get(&i) {
                    new_data.push(imgb::repack_type2(&res.data, dds, &mut new_imgb)?);
                } else {
                    new_data.push(imgb::repack_2d_passthrough(
                        &res.data,
                        original_imgb,
                        &mut new_imgb,
                    )?);
                }
            } else {
                new_data.push(res.data.clone());
            }
        }

        // Each resource keeps its original span, shifted by its size delta. A naive 16-aligned concat
        // over-pads and duplicates the pool RESOURCE_ID shares with the preceding resource.
        let n = self.resources.len();
        let declared = |i: usize| -> usize {
            if first_gtex[i].is_some() {
                new_data[i].len()
            } else {
                self.resources[i].size as usize
            }
        };
        let mut offsets = vec![0u32; n];
        let mut running = 0u32;
        for (i, off) in offsets.iter_mut().enumerate() {
            *off = running;
            let old_span = if i + 1 < n {
                self.resources[i + 1].offset - self.resources[i].offset
            } else {
                self.resources[i].size
            };
            let footprint = old_span as i64 + declared(i) as i64 - self.resources[i].size as i64;
            running += footprint.max(0) as u32;
        }

        let mut region: Vec<u8> = Vec::with_capacity(running as usize);
        for i in 0..n {
            let off = offsets[i] as usize;
            if region.len() < off + new_data[i].len() {
                region.resize(off + new_data[i].len(), 0);
            }
            region[off..off + new_data[i].len()].copy_from_slice(&new_data[i]);
        }
        // The name pool spills past the last resource's declared size, so keep the trailing region.
        let orig_end = self
            .resources
            .iter()
            .map(|r| (r.offset + r.size) as usize)
            .max()
            .unwrap_or(0);
        if orig_end < self.region.len() {
            region.extend_from_slice(&self.region[orig_end..]);
        }

        let new_ids_start = offsets
            .get(self.rid_resource)
            .map_or(self.ids_start_offset, |&o| {
                o + (self.ids_start_offset - self.rid_orig_offset)
            });

        let mut out = Vec::with_capacity(HEADER_LEN + n * 16 + region.len());
        out.extend_from_slice(&self.header);
        for (i, res) in self.resources.iter().enumerate() {
            out.extend_from_slice(&res.index.to_le_bytes());
            out.extend_from_slice(&offsets[i].to_le_bytes());
            out.extend_from_slice(&(declared(i) as u32).to_le_bytes());
            out.extend_from_slice(&res.type_val.to_le_bytes());
        }
        out.extend_from_slice(&region);

        let file_size = out.len() as u32;
        out[16..20].copy_from_slice(&file_size.to_le_bytes());
        out[52..56].copy_from_slice(&new_ids_start.to_le_bytes());
        Ok((out, new_imgb))
    }
}

/// `(marker_count, [(material_code, count)])`.
type BxtHead = (usize, Vec<([u8; 4], usize)>);

/// The head runs to the `0xFF×8` separator: `bxt\0` markers first, then 4-byte material codes.
fn parse_bxt_head(b: &[u8]) -> BxtHead {
    let ff = b.windows(8).position(|w| w == [0xff; 8]).unwrap_or(b.len());
    let head = &b[..ff];
    let mut n = 0;
    while head.get(n * 4..n * 4 + 4) == Some(b"bxt\0".as_slice()) {
        n += 1;
    }
    let mut codes: Vec<([u8; 4], usize)> = Vec::new();
    let mut i = n * 4;
    while i + 4 <= head.len() {
        let c: [u8; 4] = head[i..i + 4].try_into().unwrap();
        match codes.iter_mut().find(|(k, _)| *k == c) {
            Some((_, cnt)) => *cnt += 1,
            None => codes.push((c, 1)),
        }
        i += 4;
    }
    (n, codes)
}

/// Only the tag table is ever read back, but the trailing name pool is kept because
/// [`Trb::serialize_replacing`] overlaps `RESOURCE_ID` onto this resource by its longest suffix match.
fn build_bxt(
    target: &[u8],
    donors: &[&[u8]],
    short_slots: &[[u8; 16]],
    full_names: &[Vec<u8>],
    base_rc: usize,
    phb_count: usize,
) -> Vec<u8> {
    let (tgt_n, tgt_codes) = parse_bxt_head(target);
    let donor_parsed: Vec<BxtHead> = donors.iter().map(|d| parse_bxt_head(d)).collect();
    let donor_code = |code: &[u8; 4]| {
        donor_parsed
            .iter()
            .filter_map(|(_, cs)| cs.iter().find(|(k, _)| k == code).map(|(_, c)| *c))
            .max()
    };

    let base_ff = target.windows(8).position(|w| w == [0xff; 8]).unwrap_or(0);

    // Only the target's code types, never donor-only ones: a donor physics type driving a bone the
    // merged skeleton lacks is a dangling reference the game crashes on.
    let mut codes: Vec<([u8; 4], usize)> = Vec::new();
    for (code, tgt_cnt) in &tgt_codes {
        let cnt = if code == b"bhp\0" {
            phb_count
        } else if donors.is_empty() {
            *tgt_cnt
        } else {
            donor_code(code).unwrap_or(*tgt_cnt)
        };
        codes.push((*code, cnt));
    }

    // Count the records actually emitted below, so the game never reads a phantom chain past them.
    let n = if donors.is_empty() {
        tgt_n
    } else {
        short_slots.iter().take_while(|s| s[0] != 0).count()
    };

    let pool_len = target.len().saturating_sub(base_ff + 8 + base_rc * 16);

    let mut out = Vec::new();
    for _ in 0..n {
        out.extend_from_slice(b"bxt\0");
    }
    for (code, cnt) in &codes {
        for _ in 0..*cnt {
            out.extend_from_slice(code);
        }
    }
    out.extend_from_slice(&[0xff; 8]);
    for slot in short_slots {
        out.extend_from_slice(&[0, 0, 0, 0]);
        let name = slot.split(|&b| b == 0).next().unwrap_or(&[]);
        let mut nm = [0u8; 12];
        let k = name.len().min(12);
        nm[..k].copy_from_slice(&name[..k]);
        out.extend_from_slice(&nm);
    }
    let mut pool = vec![0u8; 4];
    for f in full_names {
        pool.extend_from_slice(f);
        pool.push(0);
    }
    pool.truncate(pool_len);
    out.extend_from_slice(&pool);
    out
}

fn replace_bytes(buf: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    if old.is_empty() || buf.len() < old.len() {
        return buf.to_vec();
    }
    let mut out = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        if buf[i..].starts_with(old) {
            out.extend_from_slice(new);
            i += old.len();
        } else {
            out.push(buf[i]);
            i += 1;
        }
    }
    out
}

fn is_fxx_prefix(name: &[u8]) -> bool {
    name.len() >= 4
        && name[0] == b'F'
        && name[1].is_ascii_hexdigit()
        && name[2].is_ascii_hexdigit()
        && name[3] == b'\\'
}

fn malformed(detail: &str) -> FormatError {
    FormatError::Malformed {
        format: "TRB",
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outfit_swap_geometry_combine() {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return;
        };
        let (Ok(s0), Ok(s1)) = (
            std::fs::read(format!("{dir}/c006/bin/c006.win32.trb")),
            std::fs::read(format!("{dir}/c206/bin/c206.win32.trb")),
        ) else {
            return;
        };
        let (t0, t1) = (Trb::parse(&s0).unwrap(), Trb::parse(&s1).unwrap());
        let is_wrb = |t: &Trb, i: usize| {
            t.resource_data(i)
                .is_some_and(|d| d.starts_with(b"SEDBwrb"))
        };
        let wrb0 = (0..t0.resource_count()).find(|&i| is_wrb(&t0, i)).unwrap();
        let wrb1 = (0..t1.resource_count()).find(|&i| is_wrb(&t1, i)).unwrap();

        let mut map: Vec<(usize, usize)> = (0..t1.resource_count()).map(|i| (1, i)).collect();
        map[wrb1] = (0, wrb0);
        let out = Trb::combine_resources(&[&t0, &t1], &map).unwrap();

        assert_eq!(out.len(), t1.resource_count());
        assert_eq!(
            out[wrb1],
            t0.resource_data(wrb0).unwrap(),
            "swapped geometry must be c006's mesh"
        );
        assert!(out[wrb1].starts_with(b"SEDBwrb"));
        for i in (0..out.len()).filter(|&i| i != wrb1) {
            assert_eq!(out[i], t1.resource_data(i).unwrap());
        }
    }

    #[test]
    fn combine_imgb_preserves_textures() {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return;
        };
        let (Ok(s0), Ok(i0)) = (
            std::fs::read(format!("{dir}/c006/bin/c006.win32.trb")),
            std::fs::read(format!("{dir}/c006/bin/c006.win32.imgb")),
        ) else {
            return;
        };
        let t0 = Trb::parse(&s0).unwrap();
        let map: Vec<(usize, usize)> = (0..t0.resource_count()).map(|i| (0, i)).collect();
        let mut res = Trb::combine_resources(&[&t0], &map).unwrap();
        let mut starts: Vec<usize> = (0..t0.resource_count())
            .filter_map(|i| {
                let d = t0.resource_data(i)?;
                let g = imgb::parse_gtex(d, *imgb::find_gtex(d).first()?).ok()?;
                g.mips.iter().map(|&(s, _)| s as usize).min()
            })
            .collect();
        starts.push(i0.len());
        starts.sort_unstable();
        let new_imgb = Trb::reassemble_imgb(&mut res, &map, &[&i0], &[starts]).unwrap();
        assert!(new_imgb.len().is_multiple_of(16));

        let mut checked = 0;
        for i in t0.texture_resources() {
            let from_combined = imgb::extract(&res[i], &new_imgb).unwrap();
            let from_source = imgb::extract(t0.resource_data(i).unwrap(), &i0).unwrap();
            assert_eq!(from_combined.len(), from_source.len());
            for (a, b) in from_combined.iter().zip(&from_source) {
                assert_eq!(a.dds, b.dds, "texture {i} pixels changed across reassembly");
            }
            checked += 1;
        }
        assert!(checked > 0, "no textures checked");
    }

    #[test]
    fn combine_model_end_to_end() {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return;
        };
        let (Ok(s), Ok(i)) = (
            std::fs::read(format!("{dir}/c006/bin/c006.win32.trb")),
            std::fs::read(format!("{dir}/c006/bin/c006.win32.imgb")),
        ) else {
            return;
        };
        let t = Trb::parse(&s).unwrap();
        let map: Vec<(usize, usize)> = (0..t.resource_count()).map(|i| (0, i)).collect();
        let (trb, imgb) = Trb::combine_model(&t, &[(&t, &i)], &map, &[]).unwrap();

        let out = Trb::parse(&trb).unwrap();
        assert_eq!(out.resource_count(), t.resource_count());
        let wrb = |x: &Trb| {
            (0..x.resource_count()).find_map(|k| {
                x.resource_data(k)
                    .filter(|d| d.starts_with(b"SEDBwrb"))
                    .map(<[u8]>::to_vec)
            })
        };
        assert_eq!(wrb(&out), wrb(&t), "combined geometry must equal source");
        for k in out.texture_resources() {
            let a = imgb::extract(out.resource_data(k).unwrap(), &imgb).unwrap();
            let b = imgb::extract(t.resource_data(k).unwrap(), &i).unwrap();
            assert_eq!(a.first().map(|x| &x.dds), b.first().map(|x| &x.dds));
        }
    }

    #[test]
    fn repack_real_roundtrip_if_present() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let fl = crate::Filelist::read(
            format!("{dir}/sys/filelist_scru.win32.bin"),
            crate::Game::XIII,
        )
        .unwrap();
        let trb_entry = fl
            .entries
            .iter()
            .find(|e| e.path.ends_with(".trb"))
            .expect(".trb");
        let imgb_path = trb_entry.path.replace(".trb", ".imgb");
        let imgb_entry = fl
            .entries
            .iter()
            .find(|e| e.path == imgb_path)
            .expect("paired .imgb");
        let mut img = std::fs::File::open(format!("{dir}/sys/white_scru.win32.bin")).unwrap();
        let trb_bytes = fl.extract(&mut img, trb_entry).unwrap();
        let imgb_bytes = fl.extract(&mut img, imgb_entry).unwrap();

        let trb = Trb::parse(&trb_bytes).unwrap();
        let original = trb.extract_textures(&imgb_bytes).unwrap();
        eprintln!("{} has {} textures", trb_entry.path, original.len());

        let map: HashMap<usize, Vec<u8>> = original.iter().cloned().collect();
        let (new_trb, new_imgb) = trb.repack(&imgb_bytes, &map).unwrap();
        let trb2 = Trb::parse(&new_trb).unwrap();
        let again = trb2.extract_textures(&new_imgb).unwrap();

        assert_eq!(original.len(), again.len(), "texture count changed");
        for ((i, a), (j, b)) in original.iter().zip(&again) {
            assert_eq!(i, j, "resource index mismatch");
            assert_eq!(a, b, "texture {i} data changed across repack");
        }
        eprintln!("TRB Type-2 round-trip OK for {} textures", original.len());
    }

    /// `ids_start` picks which entry becomes the RESOURCE_ID.
    fn build_sedbres(region: &[u8], entries: &[(u32, u32, u32)], ids_start: u32) -> Vec<u8> {
        let count = entries.len();
        let mut out = vec![0u8; HEADER_LEN + count * 16];
        out[0..8].copy_from_slice(MAGIC);
        let total = (HEADER_LEN + count * 16 + region.len()) as u32;
        out[16..20].copy_from_slice(&total.to_le_bytes());
        out[52..56].copy_from_slice(&ids_start.to_le_bytes());
        out[56..60].copy_from_slice(&(count as u32).to_le_bytes());
        for (i, &(off, sz, ty)) in entries.iter().enumerate() {
            let e = HEADER_LEN + i * 16;
            out[e..e + 4].copy_from_slice(&(i as u32).to_le_bytes());
            out[e + 4..e + 8].copy_from_slice(&off.to_le_bytes());
            out[e + 8..e + 12].copy_from_slice(&sz.to_le_bytes());
            out[e + 12..e + 16].copy_from_slice(&ty.to_le_bytes());
        }
        out.extend_from_slice(region);
        out
    }

    fn deterministic_region(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8).wrapping_mul(7).wrapping_add(3))
            .collect()
    }

    /// res2 is the RID, overlapping res1's last 16 bytes, with 8 trailing pool bytes past its end.
    fn overlapping_container() -> Vec<u8> {
        let mut region = deterministic_region(112);
        region[0..7].copy_from_slice(b"SEDBwrb"); // looks like geometry, has no GTEX
        let entries = [(0u32, 32u32, 2u32), (32, 48, 0), (64, 40, 0)];
        build_sedbres(&region, &entries, 80) // ids_start in res2's unique tail
    }

    #[test]
    fn repack_unchanged_is_byte_exact() {
        let input = overlapping_container();
        let t = Trb::parse(&input).unwrap();
        assert_eq!(t.rid_resource, 2, "RID should be the overlapping resource");
        let (out, imgb) = t.repack(&[], &HashMap::new()).unwrap();
        assert!(imgb.is_empty(), "no textures -> empty imgb");
        assert_eq!(
            out, input,
            "repack of an unchanged overlapping container must be byte-exact"
        );
    }

    #[test]
    fn repack_preserves_rid_overlap() {
        let t = Trb::parse(&overlapping_container()).unwrap();
        let (out, _) = t.repack(&[], &HashMap::new()).unwrap();
        let t2 = Trb::parse(&out).unwrap();
        let (r1s, r1e) = t2.resource_abs_span(1).unwrap();
        let (r2s, _) = t2.resource_abs_span(2).unwrap();
        assert!(
            r2s > r1s && r2s < r1e,
            "RID must overlap inside res1 ({r1s}..{r1e}), got {r2s}"
        );
    }

    #[test]
    fn serialize_replacing_same_bytes_round_trips() {
        let input = overlapping_container();
        let t = Trb::parse(&input).unwrap();
        let same = t.resource_data(0).unwrap().to_vec();
        assert_eq!(
            t.serialize_replacing(0, &same).unwrap(),
            input,
            "replacing a resource with its own bytes must round-trip"
        );
    }

    #[test]
    fn repack_keeps_over_declared_rid_size() {
        let region = deterministic_region(96);
        // res1 declares [72,112) but the region ends at 96, so only 24 bytes are stored.
        let entries = [(0u32, 72u32, 2u32), (72, 40, 0)];
        let input = build_sedbres(&region, &entries, 72);
        let t = Trb::parse(&input).unwrap();
        assert_eq!(
            t.resource_data(1).unwrap().len(),
            24,
            "stored data clamped to EOF"
        );
        let (out, _) = t.repack(&[], &HashMap::new()).unwrap();
        let t2 = Trb::parse(&out).unwrap();
        let (s, e) = t2.resource_abs_span(1).unwrap();
        assert_eq!(
            e - s,
            40,
            "over-declared RID size must be preserved, not the clamped 24"
        );
        assert_eq!(
            out, input,
            "byte-exact round-trip with an over-declared RID"
        );
    }
}
