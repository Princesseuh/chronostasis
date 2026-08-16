//! White-Engine WRB model chunk tree: the geometry container in a model `.trb`.

use byteorder::{BigEndian as BE, ByteOrder};

use crate::{FormatError, Result};

const HEADER_LEN: usize = 16;

fn align16(n: usize) -> usize {
    (n + 15) & !15
}

#[derive(Clone)]
pub struct Chunk {
    header: [u8; HEADER_LEN],
    body: Body,
    /// Zero padding up to `align(size, 16)`, kept for byte-exact output.
    pad: Vec<u8>,
}

#[derive(Clone)]
enum Body {
    Leaf(Vec<u8>),
    Container {
        info: [u8; HEADER_LEN],
        children: Vec<Chunk>,
    },
}

impl Chunk {
    pub fn magic(&self) -> [u8; 4] {
        [
            self.header[0],
            self.header[1],
            self.header[2],
            self.header[3],
        ]
    }

    /// 4-char tag with any trailing NULs trimmed (`WRB`, `MDLC`, `MESH`, …).
    pub fn tag(&self) -> String {
        String::from_utf8_lossy(&self.magic())
            .trim_end_matches('\0')
            .to_string()
    }

    fn size(&self) -> usize {
        BE::read_u32(&self.header[8..12]) as usize
    }

    pub fn children(&self) -> &[Chunk] {
        match &self.body {
            Body::Container { children, .. } => children,
            Body::Leaf(_) => &[],
        }
    }

    pub fn leaf(&self) -> Option<&[u8]> {
        match &self.body {
            Body::Leaf(d) => Some(d),
            Body::Container { .. } => None,
        }
    }

    pub fn children_mut(&mut self) -> &mut [Chunk] {
        match &mut self.body {
            Body::Container { children, .. } => children,
            Body::Leaf(_) => &mut [],
        }
    }

    /// Structural add/remove does NOT update the info-row count; re-serialize with [`serialize_recompute`].
    pub fn children_vec_mut(&mut self) -> Option<&mut Vec<Chunk>> {
        match &mut self.body {
            Body::Container { children, .. } => Some(children),
            Body::Leaf(_) => None,
        }
    }

    /// Container info row: BE `u32` sub-chunk count then 3 reserved words (leftover junk in a few
    /// hundred shipped chunks, so never assume zero).
    pub fn info_mut(&mut self) -> Option<&mut [u8; HEADER_LEN]> {
        match &mut self.body {
            Body::Container { info, .. } => Some(info),
            Body::Leaf(_) => None,
        }
    }

    /// The count the info row declares, which matches [`Chunk::children`] on every shipped chunk.
    pub fn sub_chunk_count(&self) -> Option<u32> {
        match &self.body {
            Body::Container { info, .. } => Some(BE::read_u32(&info[0..4])),
            Body::Leaf(_) => None,
        }
    }

    pub fn leaf_mut(&mut self) -> Option<&mut [u8]> {
        match &mut self.body {
            Body::Leaf(d) => Some(d),
            Body::Container { .. } => None,
        }
    }

    /// May change size, so re-serialize with [`serialize_recompute`]. No-op on a container.
    pub fn set_leaf(&mut self, data: Vec<u8>) {
        if let Body::Leaf(d) = &mut self.body {
            *d = data;
            self.pad.clear();
        }
    }

    /// Reclassifies a speculatively-parsed container (e.g. `ENVD`/`AABB`) back to a leaf.
    pub fn set_content(&mut self, data: Vec<u8>) {
        self.body = Body::Leaf(data);
        self.pad.clear();
    }

    /// `:V` edit-vertex: write `v` into this `STMS` at byte `offset` (op `a6`), encoded per `mode` (op `a5`).
    pub fn apply_vertex_edit(
        &mut self,
        vert: u32,
        offset: usize,
        mode: VertexEdit,
        v: [f32; 3],
    ) -> Option<()> {
        let (vbuf_start, stride) = {
            let s = self.as_stms()?;
            (16 + s.elements.len() * 16, s.stride as usize)
        };
        let base = vbuf_start + (vert as usize).checked_mul(stride)? + offset;
        let leaf = self.leaf_mut()?;
        match mode {
            VertexEdit::Position => {
                let slot = leaf.get_mut(base..base + 8)?;
                for k in 0..3 {
                    slot[k * 2..k * 2 + 2]
                        .copy_from_slice(&((v[k] as f64 * 32767.0) as i64 as i16).to_be_bytes());
                }
                slot[6..8].copy_from_slice(&1i16.to_be_bytes());
            }
            VertexEdit::Half2 => {
                let slot = leaf.get_mut(base..base + 4)?;
                slot[0..2].copy_from_slice(&f16_be(v[0]));
                slot[2..4].copy_from_slice(&f16_be(v[1]));
            }
            VertexEdit::Unorm2 => {
                let slot = leaf.get_mut(base..base + 2)?;
                slot[0] = (v[0] as f64 * 255.1) as i64 as u8;
                slot[1] = (v[1] as f64 * 255.1) as i64 as u8;
            }
        }
        Some(())
    }

    /// Overwrites vertex `vert`'s BE int16-SNORM position in place, leaving every size unchanged.
    pub fn set_position_raw(&mut self, vert: u32, raw: [i16; 3]) -> Option<()> {
        let (vbuf_start, off, stride) = {
            let s = self.as_stms()?;
            (
                16 + s.elements.len() * 16,
                s.position_offset()?,
                s.stride as usize,
            )
        };
        let base = vbuf_start + (vert as usize).checked_mul(stride)? + off;
        let slot = self.leaf_mut()?.get_mut(base..base + 6)?;
        slot[0..2].copy_from_slice(&raw[0].to_be_bytes());
        slot[2..4].copy_from_slice(&raw[1].to_be_bytes());
        slot[4..6].copy_from_slice(&raw[2].to_be_bytes());
        Some(())
    }

    /// Content bytes either way, since leaves like `AABB` are speculatively parsed as containers.
    pub fn content(&self) -> Vec<u8> {
        match &self.body {
            Body::Leaf(d) => d.clone(),
            Body::Container { info, children } => {
                let mut v = info.to_vec();
                for c in children {
                    c.write(&mut v);
                }
                v
            }
        }
    }

    fn content_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        match &self.body {
            Body::Leaf(d) => std::borrow::Cow::Borrowed(d),
            Body::Container { .. } => std::borrow::Cow::Owned(self.content()),
        }
    }

    pub fn as_stms(&self) -> Option<Stms<'_>> {
        if self.magic() != *b"STMS" {
            return None;
        }
        Stms::parse(self.leaf()?)
    }

    /// min/max are BE `f32` triples at content bytes 32 and 44.
    pub fn as_aabb(&self) -> Option<Aabb> {
        if self.magic() != *b"AABB" {
            return None;
        }
        let c = self.content_bytes();
        if c.len() < 56 {
            return None;
        }
        let f = |o: usize| BE::read_f32(&c[o..o + 4]);
        Some(Aabb {
            min: [f(32), f(36), f(40)],
            max: [f(44), f(48), f(52)],
        })
    }

    /// The box int16 (`kind 4`) positions dequantize INTO, not a tight bbox like [`Chunk::as_aabb`];
    /// min/max are BE `f32` triples at content bytes 0 and 12.
    pub fn as_comp(&self) -> Option<Aabb> {
        if self.magic() != *b"COMP" {
            return None;
        }
        let c = self.content_bytes();
        if c.len() < 24 {
            return None;
        }
        let f = |o: usize| BE::read_f32(&c[o..o + 4]);
        Some(Aabb {
            min: [f(0), f(4), f(8)],
            max: [f(12), f(16), f(20)],
        })
    }

    /// The `COMP` box in scope here. Environment `MESH`es carry their own; character meshes inherit
    /// one shared box from the enclosing `MDL`, so callers must walk it down the tree.
    pub fn child_comp(&self) -> Option<Aabb> {
        self.children().iter().find_map(|ch| match &ch.magic() {
            b"COMP" => ch.as_comp(),
            b"AABB" => ch.children().iter().find_map(|g| g.as_comp()),
            _ => None,
        })
    }

    pub fn submesh(&self) -> Option<Submesh<'_>> {
        let positions = self
            .children()
            .iter()
            .filter_map(|c| c.as_stms())
            .find(|s| s.position_offset().is_some())?;
        let indices = self
            .children()
            .iter()
            .filter_map(|c| c.as_stms())
            .find(|s| s.is_index_buffer());
        let extra = self
            .children()
            .iter()
            .filter_map(|c| c.as_stms())
            .filter(|s| {
                !s.external
                    && !s.is_index_buffer()
                    && s.position_offset().is_none()
                    && s.vert_count == positions.vert_count
            })
            .collect();
        let aabb = self.children().iter().find_map(|c| c.as_aabb())?;
        Some(Submesh {
            positions,
            extra,
            indices,
            aabb,
        })
    }

    /// One bone's skinning envelope. A MESH's `ENVD` children in order are its bone palette, indexed
    /// by vertex `usage 15`. The declared counts are authoritative: an odd-length index array is
    /// padded with one `0xFFFF` filler, so deriving the count from the offset gap invents an influence.
    pub fn as_envd(&self) -> Option<Envd> {
        if self.magic() != *b"ENVD" {
            return None;
        }
        let c = self.content_bytes();
        let name_off = BE::read_u16(c.get(0..2)?) as usize;
        let index_count = BE::read_u16(c.get(2..4)?) as usize;
        let index_off = BE::read_u16(c.get(4..6)?) as usize;
        let mut weight_off = BE::read_u16(c.get(6..8)?) as usize;
        let weight_count = BE::read_u16(c.get(8..10)?) as usize;
        if name_off < 16 {
            return None;
        }
        let raw_name = c.get(name_off..index_off)?;
        let name_len = raw_name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(raw_name.len());
        let n = index_count.min(weight_count);
        let idx = c.get(index_off..index_off + n * 2)?;
        // Offsets are u16, so in an envelope past 64 KiB the weight offset wraps around.
        while weight_off < index_off + n * 2 {
            weight_off += 0x1_0000;
        }
        let wts = c.get(weight_off..weight_off + n)?;
        let influences = (0..n)
            .map(|i| (BE::read_u16(&idx[i * 2..]), wts[i]))
            .collect();
        Some(Envd {
            bone: String::from_utf8_lossy(&raw_name[..name_len]).to_string(),
            influences,
        })
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.header);
        match &self.body {
            Body::Leaf(data) => out.extend_from_slice(data),
            Body::Container { info, children } => {
                out.extend_from_slice(info);
                for c in children {
                    c.write(out);
                }
            }
        }
        out.extend_from_slice(&self.pad);
    }

    fn write_recompute(&self, out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(&self.header);
        match &self.body {
            Body::Leaf(data) => out.extend_from_slice(data),
            Body::Container { info, children } => {
                out.extend_from_slice(info);
                for c in children {
                    c.write_recompute(out);
                }
            }
        }
        let size = out.len() - start;
        let orig_size = BE::read_u32(&self.header[8..12]) as usize;
        let orig_next_sibling = BE::read_u32(&self.header[12..16]) as usize;
        BE::write_u32(&mut out[start + 8..start + 12], size as u32);
        // The next-sibling offset stays 0 on a last sibling, and stays the stride everywhere else.
        if orig_next_sibling == align16(orig_size) {
            BE::write_u32(&mut out[start + 12..start + 16], align16(size) as u32);
        }
        out.resize(start + align16(size), 0);
    }
}

/// A world-space box, used both for a submesh's tight `AABB` and for the `COMP` dequant box.
#[derive(Debug, Clone, Copy)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    /// Lerps a `[-1, 1]` coord across `min..max`, which with the `COMP` box is the int16 dequant.
    pub fn denorm(&self, n: [f32; 3]) -> [f32; 3] {
        let mut w = [0.0f32; 3];
        for k in 0..3 {
            let span = self.max[k] - self.min[k];
            w[k] = (n[k] * 0.5 + 0.5) * span + self.min[k];
        }
        w
    }
}

/// One drawable submesh of a `MESH`.
pub struct Submesh<'a> {
    pub positions: Stms<'a>,
    /// Further streams over the same vertices: XIII-2/LR split texcoords and colour out of `positions`.
    pub extra: Vec<Stms<'a>>,
    pub indices: Option<Stms<'a>>,
    pub aabb: Aabb,
}

impl<'a> Submesh<'a> {
    pub fn stream_with(&self, usage: u8) -> Option<&Stms<'a>> {
        std::iter::once(&self.positions)
            .chain(self.extra.iter())
            .find(|s| s.elements.iter().any(|e| e.usage == usage))
    }

    /// `dequant` must be the model's shared `COMP` box, NOT this submesh's tight
    /// [`aabb`](Submesh::aabb); the shared box is what keeps a model's parts aligned.
    pub fn position_world(&self, i: u32, dequant: &Aabb) -> Option<[f32; 3]> {
        let n = self.positions.position_norm(i)?;
        let is_float = self.positions.element(0).is_some_and(|e| e.is_float());
        Some(if is_float { n } else { dequant.denorm(n) })
    }
}

pub struct Envd {
    pub bone: String,
    pub influences: Vec<(u16, u8)>,
}

/// The `a5` modes shipped mods use; the read-modify-write variants are unimplemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexEdit {
    /// `a5=0`: position. `x,y,z` BE int16 `(int)(v·32767)`, then `w=1`. 8 bytes (`a6`=0).
    Position,
    /// `a5=3`: two BE float16s `(x,y)`. 4 bytes (texcoords).
    Half2,
    /// `a5=4`: two u8s `(int)(v·255.1)` `(x,y)`. 2 bytes (bone weights).
    Unorm2,
}

/// One entry of a vertex declaration: where a semantic sits in each vertex and in what type.
#[derive(Debug, Clone, Copy)]
pub struct VertexElement {
    pub offset: u16,
    /// Type code of one component: `0` UInt16, `1` Float32, `2` Float16, `3` UByte, `4` Int16, `6` SByte.
    pub kind: u8,
    pub count: u8,
    /// Semantic usage (0 = position, 8 = texcoord, …).
    pub usage: u8,
}

impl VertexElement {
    /// Unknown type codes fall back to 4 bytes.
    pub fn component_size(&self) -> usize {
        match self.kind {
            3 | 6 => 1,
            0 | 2 | 4 => 2,
            _ => 4,
        }
    }

    pub fn size(&self) -> usize {
        self.component_size() * self.count as usize
    }

    fn is_float(&self) -> bool {
        matches!(self.kind, 1 | 2)
    }
}

/// Decoded `STMS` submesh: vertex declaration plus interleaved vertex buffer.
pub struct Stms<'a> {
    pub vert_count: u32,
    pub stride: u32,
    pub elements: Vec<VertexElement>,
    pub vertices: &'a [u8],
    /// The vertex buffer lives in another TRB or the IMGB, so `vertices` is empty.
    pub external: bool,
    /// Packed MDL/MESH/STMS identifier, 0 in every PC FFXIII asset.
    pub mesh_id: u16,
}

impl<'a> Stms<'a> {
    fn parse(leaf: &'a [u8]) -> Option<Stms<'a>> {
        if leaf.len() < 16 {
            return None;
        }
        // Header sub-fields are u16/u32, not three u32s; high bytes happen to be zero on PC assets.
        let external = BE::read_u16(&leaf[0..2]) == 0xFFFF;
        let elem_count = BE::read_u16(&leaf[2..4]) as usize;
        let vert_count = BE::read_u32(&leaf[4..8]);
        let mesh_id = BE::read_u16(&leaf[8..10]);
        let stride = BE::read_u16(&leaf[10..12]) as u32;
        let decl_end = 16 + elem_count.checked_mul(16)?;
        if leaf.len() < decl_end {
            return None;
        }
        let vbuf_len = (vert_count as usize).checked_mul(stride as usize)?;
        if !external && decl_end.checked_add(vbuf_len)? != leaf.len() {
            return None;
        }
        let elements = (0..elem_count)
            .map(|i| {
                let e = 16 + i * 16;
                VertexElement {
                    offset: BE::read_u32(&leaf[e..e + 4]) as u16,
                    kind: leaf[e + 7],
                    count: leaf[e + 11],
                    usage: (BE::read_u32(&leaf[e + 12..e + 16]) >> 16) as u8,
                }
            })
            .collect();
        Some(Stms {
            vert_count,
            stride,
            elements,
            vertices: leaf.get(decl_end..).unwrap_or(&[]),
            external,
            mesh_id,
        })
    }

    pub fn position_offset(&self) -> Option<usize> {
        self.element(0).map(|e| e.offset as usize)
    }

    pub fn element(&self, usage: u8) -> Option<&VertexElement> {
        self.elements.iter().find(|e| e.usage == usage)
    }

    fn component_bytes(&self, e: &VertexElement, vert: u32, i: usize) -> Option<&[u8]> {
        if i >= e.count as usize {
            return None;
        }
        let sz = e.component_size();
        let base = (vert as usize).checked_mul(self.stride as usize)?
            + e.offset as usize
            + i.checked_mul(sz)?;
        self.vertices.get(base..base + sz)
    }

    fn component(&self, e: &VertexElement, vert: u32, i: usize) -> Option<f32> {
        let b = self.component_bytes(e, vert, i)?;
        Some(match e.kind {
            0 => u16::from_be_bytes([b[0], b[1]]) as f32,
            2 => half_be(b[0], b[1]),
            3 => b[0] as f32,
            4 => i16::from_be_bytes([b[0], b[1]]) as f32,
            6 => b[0] as i8 as f32,
            _ => f32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        })
    }

    fn component_unorm(&self, e: &VertexElement, vert: u32, i: usize) -> Option<f32> {
        let v = self.component(e, vert, i)?;
        Some(match e.kind {
            0 => v / 65535.0,
            3 => v / 255.0,
            4 => (v + 32768.0) / 65535.0,
            6 => (v + 128.0) / 255.0,
            _ => v,
        })
    }

    /// Byte normals/tangents are UNSIGNED; reading them signed gives blotchy, seam-split shading.
    fn component_snorm(&self, e: &VertexElement, vert: u32, i: usize) -> Option<f32> {
        let v = self.component_unorm(e, vert, i)?;
        Some(if e.is_float() { v } else { v * 2.0 - 1.0 })
    }

    fn vec3_snorm(&self, usage: u8, vert: u32) -> Option<[f32; 3]> {
        let e = self.element(usage)?;
        Some([
            self.component_snorm(e, vert, 0)?,
            self.component_snorm(e, vert, 1)?,
            self.component_snorm(e, vert, 2)?,
        ])
    }

    /// Vertex normal (`usage 2`), `[-1, 1]`-mapped.
    pub fn normal(&self, vert: u32) -> Option<[f32; 3]> {
        self.vec3_snorm(2, vert)
    }

    /// Tangent (`usage 13`) plus the handedness sign its 4th component carries.
    pub fn tangent(&self, vert: u32) -> Option<([f32; 3], bool)> {
        let t = self.vec3_snorm(13, vert)?;
        let e = self.element(13)?;
        let sign = e.count < 4 || self.component_snorm(e, vert, 3)? > 0.0;
        Some((t, sign))
    }

    /// Set 0 is the primary `usage 8`, 1..=3 the `usage 9..11` lightmap/detail channels. Integer UV
    /// types, which FFXIII never ships, decode raw, since their scale is unconfirmed.
    pub fn uv_set(&self, set: u8, vert: u32) -> Option<[f32; 2]> {
        let e = self.element(8 + set)?;
        Some([self.component(e, vert, 0)?, self.component(e, vert, 1)?])
    }

    /// Primary texture coordinate (`usage 8`), BE float16 `(u, v)`.
    pub fn uv(&self, vert: u32) -> Option<[f32; 2]> {
        self.uv_set(0, vert)
    }

    /// Vertex color (`usage 3`). `kind 2` streams are read directly, so HDR colors may exceed 1.0;
    /// components the declaration omits default to 1.0.
    pub fn color_f32(&self, vert: u32) -> Option<[f32; 4]> {
        let e = self.element(3)?;
        let mut rgba = [1.0f32; 4];
        for (i, c) in rgba.iter_mut().enumerate() {
            match self.component_unorm(e, vert, i) {
                Some(v) => *c = v,
                None if i >= e.count as usize => break,
                None => return None,
            }
        }
        Some(rgba)
    }

    /// Bone-palette indices (`usage 15`), `0xFF` for an empty slot. `None` for index types wider than
    /// a byte, which the returned array cannot hold.
    pub fn bone_indices(&self, vert: u32) -> Option<[u8; 4]> {
        let e = self.element(15)?;
        if e.component_size() != 1 {
            return None;
        }
        let mut idx = [0xFFu8; 4];
        for (i, slot) in idx.iter_mut().enumerate() {
            match self.component_bytes(e, vert, i) {
                Some(b) => *slot = b[0],
                None if i >= e.count as usize => break,
                None => return None,
            }
        }
        Some(idx)
    }

    /// Bone weights (`usage 14`), `[0, 1]`-mapped and summing to 1; undeclared slots are 0.
    pub fn bone_weights(&self, vert: u32) -> Option<[f32; 4]> {
        let e = self.element(14)?;
        let mut w = [0.0f32; 4];
        for (i, slot) in w.iter_mut().enumerate() {
            match self.component_unorm(e, vert, i) {
                Some(v) => *slot = v,
                None if i >= e.count as usize => break,
                None => return None,
            }
        }
        Some(w)
    }

    /// `kind 4` only; `kind 1` float32 positions read as garbage here, so decoders want
    /// [`Stms::position_norm`], which handles both.
    pub fn position_raw(&self, i: u32) -> Option<[i16; 3]> {
        let base = (i as usize).checked_mul(self.stride as usize)? + self.position_offset()?;
        let v = self.vertices.get(base..base + 6)?;
        Some([
            i16::from_be_bytes([v[0], v[1]]),
            i16::from_be_bytes([v[2], v[3]]),
            i16::from_be_bytes([v[4], v[5]]),
        ])
    }

    /// On-disk normalized position, the value `:V` edits work in. `kind 4` is int16 SNORM `[-1,1]`,
    /// NOT world space (dequantize through the `COMP` box, see [`Submesh::position_world`]); `kind 1`
    /// is float32 world coords. int16 divides by 32767, not the generic 32767.5.
    pub fn position_norm(&self, i: u32) -> Option<[f32; 3]> {
        let e = self.element(0)?;
        let mut p = [0.0f32; 3];
        for (k, c) in p.iter_mut().enumerate() {
            *c = match e.kind {
                4 => self.component(e, i, k)? / 32767.0,
                _ => self.component_snorm(e, i, k)?,
            };
        }
        Some(p)
    }

    pub fn is_index_buffer(&self) -> bool {
        self.stride == 2 && self.elements.len() == 1 && self.elements[0].usage == 255
    }

    /// Triangle-list indices, big-endian `u16`.
    pub fn indices(&self) -> Option<Vec<u16>> {
        if !self.is_index_buffer() {
            return None;
        }
        let n = self.vert_count as usize;
        // External index buffers keep vert_count but store no bytes here.
        if self.vertices.len() < n.checked_mul(2)? {
            return None;
        }
        Some(
            (0..n)
                .map(|i| u16::from_be_bytes([self.vertices[i * 2], self.vertices[i * 2 + 1]]))
                .collect(),
        )
    }
}

/// Saturates to `±0x7bff` rather than infinity, as the game's own converter does.
/// It also emits `0x0800` for exactly `0.0`; no mod feeds it zero, so we flush to a clean `±0` instead.
pub fn f16_be(x: f32) -> [u8; 2] {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32 - 127;
    let mant = bits & 0x7f_ffff;
    let h: u16 = if bits & 0x7fff_ffff == 0 {
        sign
    } else if exp > 15 {
        sign | 0x7bff
    } else if exp < -14 {
        let shift = 13 + (-14 - exp) as u32;
        if shift > 24 {
            sign
        } else {
            sign | (((0x80_0000 | mant) + (1 << (shift - 1))) >> shift) as u16
        }
    } else {
        let m = (mant + 0x1000) >> 13;
        let (mut e, mut m) = ((exp + 15) as u16, m as u16);
        if m == 0x400 {
            e += 1;
            m = 0;
        }
        if e >= 0x1f {
            sign | 0x7bff
        } else {
            sign | (e << 10) | m
        }
    };
    h.to_be_bytes()
}

pub fn half_be(b0: u8, b1: u8) -> f32 {
    let h = ((b0 as u32) << 8) | b1 as u32;
    let sign = (h & 0x8000) << 16;
    let exp = (h >> 10) & 0x1f;
    let mant = h & 0x3ff;
    let bits = match exp {
        0 if mant == 0 => sign,
        0 => {
            let mut e = -1i32;
            let mut m = mant;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | (((127 - 15 + 2 + e) as u32) << 23) | ((m & 0x3ff) << 13)
        }
        0x1f => sign | 0x7f80_0000 | (mant << 13),
        _ => sign | ((exp + (127 - 15)) << 23) | (mant << 13),
    };
    f32::from_bits(bits)
}

fn is_ascii_magic(b: &[u8]) -> bool {
    b.len() == 4
        && b[0].is_ascii_alphabetic()
        && b.iter().all(|&c| c == 0 || (32..127).contains(&c))
}

/// Deep enough for every shipped asset; a self-referencing tree must not blow the stack.
const MAX_DEPTH: usize = 64;

/// Returns the chunk at `off` and the offset of its next sibling.
fn parse_chunk(d: &[u8], off: usize, depth: usize) -> Result<(Chunk, usize)> {
    if depth > MAX_DEPTH {
        return Err(malformed("chunk tree too deep"));
    }
    let hdr = d
        .get(off..off + HEADER_LEN)
        .ok_or_else(|| malformed("chunk header out of range"))?;
    if !is_ascii_magic(&hdr[0..4]) {
        return Err(malformed("not a chunk magic"));
    }
    let size = BE::read_u32(&hdr[8..12]) as usize;
    if size < HEADER_LEN {
        return Err(malformed("chunk size smaller than header"));
    }
    let pad_end = off + align16(size);
    let content_end = off + size;
    if pad_end > d.len() {
        return Err(malformed("chunk extends past end"));
    }

    // Nothing marks a chunk as a container, so guess; either body re-serializes to the same bytes.
    let body = (size > 2 * HEADER_LEN
        && is_ascii_magic(&d[off + 2 * HEADER_LEN..off + 2 * HEADER_LEN + 4]))
    .then(|| try_children(d, off + 2 * HEADER_LEN, content_end, depth + 1))
    .flatten()
    .map(|children| {
        let mut info = [0u8; HEADER_LEN];
        info.copy_from_slice(&d[off + HEADER_LEN..off + 2 * HEADER_LEN]);
        Body::Container { info, children }
    })
    .unwrap_or_else(|| Body::Leaf(d[off + HEADER_LEN..content_end].to_vec()));

    let mut header = [0u8; HEADER_LEN];
    header.copy_from_slice(&hdr[..HEADER_LEN]);
    Ok((
        Chunk {
            header,
            body,
            pad: d[content_end..pad_end].to_vec(),
        },
        pad_end,
    ))
}

/// `None` unless the chunks tile `[start, end)` exactly, which is what rules out leaf payloads.
fn try_children(d: &[u8], start: usize, end: usize, depth: usize) -> Option<Vec<Chunk>> {
    let mut children = Vec::new();
    let mut o = start;
    while o < end {
        let (c, next) = parse_chunk(d, o, depth).ok()?;
        if next > end {
            return None;
        }
        children.push(c);
        o = next;
    }
    (o == end).then_some(children)
}

/// `data` is a whole `SEDBwrb` resource; the chunk tree starts after its 48-byte `SEDB` envelope.
pub fn parse(data: &[u8]) -> Result<Chunk> {
    if data.get(0..4) != Some(b"SEDB") || data.get(4..7) != Some(b"wrb") {
        return Err(malformed("not a SEDBwrb resource"));
    }
    let (chunk, _) = parse_chunk(data, 48, 0)?;
    Ok(chunk)
}

/// `envelope` is the original resource, whose 48-byte `SEDB` header is copied through verbatim.
pub fn serialize(envelope: &[u8], root: &Chunk) -> Vec<u8> {
    let mut out = Vec::with_capacity(48 + root.size());
    out.extend_from_slice(&envelope[..48.min(envelope.len())]);
    root.write(&mut out);
    out
}

/// Recomputes every chunk size and the envelope total, so a structurally-edited tree stays consistent.
pub fn serialize_recompute(envelope: &[u8], root: &Chunk) -> Vec<u8> {
    use byteorder::LittleEndian as LE;
    let mut out = Vec::with_capacity(48 + root.size());
    out.extend_from_slice(&envelope[..48.min(envelope.len())]);
    root.write_recompute(&mut out);
    let total = out.len() as u32;
    if out.len() >= 20 {
        LE::write_u32(&mut out[16..20], total);
    }
    out
}

fn malformed(detail: &str) -> FormatError {
    FormatError::Malformed {
        format: "WRB",
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_be_matches_engine() {
        // Expected halves captured from real mode-3 `:V` writes.
        let cases = [
            (0.001f32, [0x14, 0x19]),
            (0.002, [0x18, 0x19]),
            (0.781, [0x3a, 0x3f]),
            (0.5, [0x38, 0x00]),
            (1.0, [0x3c, 0x00]),
            (2.5, [0x41, 0x00]),
            (-0.5, [0xb8, 0x00]),
            (0.0001, [0x06, 0x8e]),
            (0.333333, [0x35, 0x55]),
            (0.0, [0x00, 0x00]),
            (100.0, [0x56, 0x40]),
            (100000.0, [0x7b, 0xff]), // overflow saturates, not inf
        ];
        for (x, want) in cases {
            assert_eq!(f16_be(x), want, "f16_be({x})");
        }
        for &x in &[0.001f32, 0.123, 0.781, -0.05, 0.456, 12.5] {
            let b = f16_be(x);
            let back = half_be(b[0], b[1]);
            assert!(
                (back - x).abs() <= x.abs() * 0.001 + 1e-4,
                "round-trip {x} -> {back}"
            );
        }
    }

    #[test]
    fn wrb_round_trips_byte_exact() {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return;
        };
        let mut tested = 0;
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|e| e.to_str()) != Some("trb") {
                    continue;
                }
                let trb = std::fs::read(&p).unwrap();
                for res in trb_wrb_resources(&trb) {
                    let root = parse(res).expect("parse WRB");
                    let back = serialize(res, &root);
                    assert_eq!(back, res, "WRB round-trip mismatch in {}", p.display());
                    tested += 1;
                }
            }
        }
        eprintln!("round-tripped {tested} WRB resources");
        assert!(
            tested > 0,
            "no SEDBwrb resources found under FF13_MODELS_DIR"
        );
    }

    fn trb_wrb_resources(trb: &[u8]) -> Vec<&[u8]> {
        use byteorder::LittleEndian as LE;
        let mut out = Vec::new();
        if trb.len() < 64 || &trb[..8] != b"SEDBRES " {
            return out;
        }
        let rc = LE::read_u32(&trb[56..60]) as usize;
        let ds = 64 + rc * 16;
        for i in 0..rc {
            let e = 64 + i * 16;
            let off = LE::read_u32(&trb[e + 4..e + 8]) as usize;
            let size = LE::read_u32(&trb[e + 8..e + 12]) as usize;
            if let Some(d) = trb.get(ds + off..ds + off + size)
                && d.len() > 7
                && &d[..7] == b"SEDBwrb"
            {
                out.push(d);
            }
        }
        out
    }

    fn collect_stms<'a>(c: &'a Chunk, out: &mut Vec<Stms<'a>>) {
        if let Some(s) = c.as_stms() {
            out.push(s);
        }
        for child in c.children() {
            collect_stms(child, out);
        }
    }

    #[test]
    fn stms_geometry_parses() {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return;
        };
        let (mut submeshes, mut with_pos, mut verts) = (0u64, 0u64, 0u64);
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|e| e.to_str()) != Some("trb") {
                    continue;
                }
                let trb = std::fs::read(&p).unwrap();
                for res in trb_wrb_resources(&trb) {
                    let root = parse(res).unwrap();
                    let mut all = Vec::new();
                    collect_stms(&root, &mut all);
                    for s in &all {
                        submeshes += 1;
                        if s.position_offset().is_some() {
                            with_pos += 1;
                            verts += s.vert_count as u64;
                            if s.vert_count > 0 {
                                assert!(
                                    s.position_raw(s.vert_count - 1).is_some(),
                                    "position layout unsound in {}",
                                    p.display()
                                );
                            }
                        }
                    }
                }
            }
        }
        eprintln!("parsed {submeshes} STMS chunks, {with_pos} position streams, {verts} vertices");
        assert!(with_pos > 0, "no STMS position geometry found");
    }

    fn grow_first_leaf(c: &mut Chunk, extra: &[u8]) -> bool {
        let grown = c.leaf().map(|l| {
            let mut v = l.to_vec();
            v.extend_from_slice(extra);
            v
        });
        if let Some(v) = grown {
            c.set_leaf(v);
            return true;
        }
        for k in c.children_mut() {
            if grow_first_leaf(k, extra) {
                return true;
            }
        }
        false
    }

    #[test]
    fn structural_recompute() {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return;
        };
        let mut tested = 0;
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|e| e.to_str()) != Some("trb") {
                    continue;
                }
                let trb = std::fs::read(&p).unwrap();
                for res in trb_wrb_resources(&trb) {
                    let root = parse(res).unwrap();
                    assert_eq!(
                        serialize_recompute(res, &root),
                        res,
                        "recompute drift in {}",
                        p.display()
                    );
                    let mut edited = parse(res).unwrap();
                    assert!(grow_first_leaf(&mut edited, &[0u8; 24]));
                    let out = serialize_recompute(res, &edited);
                    assert_eq!(out.len() % 16, 0, "unaligned output in {}", p.display());
                    parse(&out).expect("edited WRB re-parses");
                    tested += 1;
                }
            }
        }
        eprintln!("structural-recompute on {tested} WRBs");
        assert!(tested > 0);
    }

    fn edit_first_vertex(c: &mut Chunk, raw: [i16; 3]) -> bool {
        if c.as_stms().is_some_and(|s| s.position_offset().is_some()) {
            return c.set_position_raw(0, raw).is_some();
        }
        for k in c.children_mut() {
            if edit_first_vertex(k, raw) {
                return true;
            }
        }
        false
    }

    fn first_position_raw(c: &Chunk) -> Option<[i16; 3]> {
        if let Some(s) = c.as_stms()
            && s.position_offset().is_some()
        {
            return s.position_raw(0);
        }
        c.children().iter().find_map(first_position_raw)
    }

    #[test]
    fn vertex_edit_round_trips() {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return;
        };
        let mut edited = 0;
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|e| e.to_str()) != Some("trb") {
                    continue;
                }
                let trb = std::fs::read(&p).unwrap();
                for res in trb_wrb_resources(&trb) {
                    let mut root = parse(res).unwrap();
                    let new_raw = [1234i16, -5678, 9012];
                    if !edit_first_vertex(&mut root, new_raw) {
                        continue;
                    }
                    let out = serialize(res, &root);
                    assert_eq!(
                        out.len(),
                        res.len(),
                        "edit changed file length in {}",
                        p.display()
                    );
                    let reparsed = parse(&out).unwrap();
                    assert_eq!(
                        first_position_raw(&reparsed),
                        Some(new_raw),
                        "edit lost in {}",
                        p.display()
                    );
                    edited += 1;
                }
            }
        }
        eprintln!("edited+round-tripped {edited} models");
        assert!(edited > 0);
    }

    #[test]
    fn vertex_elements_decode() {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return;
        };
        let mut checked = 0u64;
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|e| e.to_str()) != Some("trb") {
                    continue;
                }
                let trb = std::fs::read(&p).unwrap();
                for res in trb_wrb_resources(&trb) {
                    let root = parse(res).unwrap();
                    let mut all = Vec::new();
                    collect_stms(&root, &mut all);
                    for s in all.iter().filter(|s| s.position_offset().is_some()) {
                        for v in (0..s.vert_count).step_by(97) {
                            if let Some(w) = s.bone_weights(v) {
                                let sum: f32 = w.iter().sum();
                                assert!(
                                    (sum - 1.0).abs() < 0.02,
                                    "weights sum {sum} in {}",
                                    p.display()
                                );
                            }
                            if let Some(n) = s.normal(v) {
                                assert!(
                                    n.iter().all(|&c| (-1.01..=1.01).contains(&c)),
                                    "normal {n:?} in {}",
                                    p.display()
                                );
                            }
                            if let Some(uv) = s.uv(v) {
                                assert!(uv[0].is_finite() && uv[1].is_finite());
                            }
                            checked += 1;
                        }
                    }
                }
            }
        }
        eprintln!("checked vertex elements on {checked} vertices");
        assert!(checked > 0);
    }

    #[test]
    fn envd_matches_vertex_skinning() {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return;
        };
        let (mut hit, mut miss) = (0u64, 0u64);
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|e| e.to_str()) != Some("trb") {
                    continue;
                }
                let trb = std::fs::read(&p).unwrap();
                for res in trb_wrb_resources(&trb) {
                    let root = parse(res).unwrap();
                    let mut meshes = Vec::new();
                    collect_meshes(&root, &mut meshes);
                    for m in meshes {
                        let palette: Vec<Envd> =
                            m.children().iter().filter_map(|c| c.as_envd()).collect();
                        let Some(stms) = m
                            .children()
                            .iter()
                            .filter_map(|c| c.as_stms())
                            .find(|s| s.position_offset().is_some())
                        else {
                            continue;
                        };
                        if palette.is_empty() || stms.bone_indices(0).is_none() {
                            continue;
                        }
                        // Some weapon rigs index a different palette, so their ENVDs don't address this stream.
                        if palette.iter().any(|e| {
                            e.influences
                                .iter()
                                .any(|&(vi, _)| (vi as u32) >= stms.vert_count)
                        }) {
                            continue;
                        }
                        for v in (0..stms.vert_count).step_by(53) {
                            let idx = stms.bone_indices(v).unwrap();
                            let wt = stms.bone_weights(v).unwrap();
                            for k in 0..4 {
                                if idx[k] == 0xff || (wt[k] * 255.0).round() as u8 == 0 {
                                    continue;
                                }
                                let envd_w = palette
                                    .get(idx[k] as usize)
                                    .and_then(|b| {
                                        b.influences.iter().find(|&&(vi, _)| vi as u32 == v)
                                    })
                                    .map(|&(_, w)| w);
                                if envd_w == Some((wt[k] * 255.0).round() as u8) {
                                    hit += 1;
                                } else {
                                    miss += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        let rate = hit as f64 / (hit + miss).max(1) as f64;
        eprintln!(
            "skinning cross-check: {hit} hit, {miss} miss ({:.1}% consistent)",
            rate * 100.0
        );
        assert!(
            hit > 0 && rate > 0.999,
            "skinning consistency too low: {rate}"
        );
    }

    fn collect_meshes<'a>(c: &'a Chunk, out: &mut Vec<&'a Chunk>) {
        if c.magic() == *b"MESH" {
            out.push(c);
        }
        for child in c.children() {
            collect_meshes(child, out);
        }
    }

    fn corpus_trbs() -> Option<Vec<std::path::PathBuf>> {
        let dir = std::env::var("FF13_MODELS_DIR").ok()?;
        let mut out = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|e| e.to_str()) == Some("trb") {
                    out.push(p);
                }
            }
        }
        Some(out)
    }

    fn walk_chunks<'a>(c: &'a Chunk, out: &mut Vec<&'a Chunk>) {
        out.push(c);
        for child in c.children() {
            walk_chunks(child, out);
        }
    }

    #[test]
    fn vertex_declarations_are_self_describing() {
        let Some(paths) = corpus_trbs() else { return };
        let mut seen: std::collections::BTreeMap<(u8, u8, u8), u64> = Default::default();
        for p in paths {
            let trb = std::fs::read(&p).unwrap();
            for res in trb_wrb_resources(&trb) {
                let root = parse(res).unwrap();
                let mut all = Vec::new();
                collect_stms(&root, &mut all);
                for s in &all {
                    for e in &s.elements {
                        assert!(
                            matches!(e.kind, 0 | 1 | 2 | 3 | 4 | 6),
                            "unknown vertex data type {} in {}",
                            e.kind,
                            p.display()
                        );
                        assert!(e.count >= 1, "zero-component element in {}", p.display());
                        assert!(
                            e.offset as usize + e.size() <= s.stride as usize,
                            "element {e:?} overruns stride {} in {}",
                            s.stride,
                            p.display()
                        );
                        let needed = match e.usage {
                            0 | 2 | 4 | 13 => 3,
                            8..=11 => 2,
                            3 | 14 | 15 => 4,
                            _ => 1,
                        };
                        assert!(
                            e.count >= needed,
                            "element {e:?} has fewer components than usage {} needs in {}",
                            e.usage,
                            p.display()
                        );
                        *seen.entry((e.usage, e.kind, e.count)).or_default() += 1;
                    }
                }
            }
        }
        eprintln!("(usage, type, count) combinations: {seen:?}");
        assert!(!seen.is_empty(), "no vertex declarations found");
    }

    #[test]
    fn envd_influences_follow_declared_counts() {
        let Some(paths) = corpus_trbs() else { return };
        let (mut total, mut padded) = (0u64, 0u64);
        for p in paths {
            let trb = std::fs::read(&p).unwrap();
            for res in trb_wrb_resources(&trb) {
                let root = parse(res).unwrap();
                let mut all = Vec::new();
                walk_chunks(&root, &mut all);
                for c in all.iter().filter(|c| c.magic() == *b"ENVD") {
                    let content = c.content();
                    let declared = BE::read_u16(&content[2..4]) as usize;
                    let envd = c.as_envd().expect("ENVD decodes");
                    assert_eq!(
                        envd.influences.len(),
                        declared,
                        "influence count differs from the header in {}",
                        p.display()
                    );
                    assert!(
                        envd.influences.iter().all(|&(v, _)| v != 0xFFFF),
                        "index-array filler decoded as an influence in {}",
                        p.display()
                    );
                    assert!(!envd.bone.is_empty(), "unnamed ENVD in {}", p.display());
                    total += declared as u64;
                    padded += (declared % 2) as u64;
                }
            }
        }
        eprintln!("{total} ENVD influences, {padded} envelopes with a filler slot");
        assert!(total > 0, "no ENVD envelopes found");
    }

    #[test]
    fn container_bookkeeping_matches_the_tree() {
        let Some(paths) = corpus_trbs() else { return };
        let (mut containers, mut last) = (0u64, 0u64);
        for p in paths {
            let trb = std::fs::read(&p).unwrap();
            for res in trb_wrb_resources(&trb) {
                let root = parse(res).unwrap();
                let mut all = Vec::new();
                walk_chunks(&root, &mut all);
                for c in &all {
                    if let Some(n) = c.sub_chunk_count() {
                        assert_eq!(
                            n as usize,
                            c.children().len(),
                            "info-row child count differs from the tree in {}",
                            p.display()
                        );
                        containers += 1;
                    }
                    let kids = c.children();
                    for (i, k) in kids.iter().enumerate() {
                        let next = BE::read_u32(&k.header[12..16]) as usize;
                        if i + 1 == kids.len() {
                            assert_eq!(next, 0, "last sibling links on in {}", p.display());
                            last += 1;
                        } else {
                            assert_eq!(
                                next,
                                align16(k.size()),
                                "sibling offset is not the chunk stride in {}",
                                p.display()
                            );
                        }
                    }
                }
            }
        }
        eprintln!("{containers} containers, {last} sibling lists");
        assert!(containers > 0, "no containers found");
    }

    fn collect_meshes_comp<'a>(
        c: &'a Chunk,
        inherited: Option<Aabb>,
        out: &mut Vec<(&'a Chunk, Option<Aabb>)>,
    ) {
        let eff = c.child_comp().or(inherited);
        if c.magic() == *b"MESH" {
            out.push((c, eff));
        }
        for child in c.children() {
            collect_meshes_comp(child, eff, out);
        }
    }

    #[test]
    fn submesh_positions_snorm_roundtrip() {
        let Ok(dir) = std::env::var("FF13_MODELS_DIR") else {
            return;
        };
        let mut checked = 0u64;
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|e| e.to_str()) != Some("trb") {
                    continue;
                }
                let trb = std::fs::read(&p).unwrap();
                for res in trb_wrb_resources(&trb) {
                    let root = parse(res).unwrap();
                    let mut meshes = Vec::new();
                    collect_meshes_comp(&root, None, &mut meshes);
                    for (m, comp) in meshes {
                        let Some(sm) = m.submesh() else { continue };
                        let (stms, aabb) = (&sm.positions, sm.aabb);
                        if stms.vert_count == 0 {
                            continue;
                        }
                        if let Some(idx) = sm.indices.as_ref().and_then(|s| s.indices()) {
                            assert_eq!(
                                idx.len() % 3,
                                0,
                                "non-triangle index count in {}",
                                p.display()
                            );
                            assert!(
                                idx.iter().all(|&i| (i as u32) < stms.vert_count),
                                "index out of range in {}",
                                p.display()
                            );
                        }
                        for k in 0..3 {
                            assert!(
                                aabb.min[k].is_finite() && aabb.min[k] <= aabb.max[k],
                                "bad AABB {aabb:?} in {}",
                                p.display()
                            );
                        }
                        let pe = stms.elements.iter().find(|e| e.usage == 0).unwrap();
                        let off = pe.offset as usize;
                        let stride = stms.stride as usize;
                        let snorm = pe.kind != 1;
                        // The AABB is a culling hint, not a tight bound, so vertices sit slightly outside it.
                        let margin = (0..3)
                            .map(|k| aabb.max[k] - aabb.min[k])
                            .fold(1.0f32, f32::max)
                            * 2.0;
                        for i in 0..stms.vert_count {
                            let n = stms.position_norm(i).unwrap();
                            for k in 0..3 {
                                assert!(
                                    n[k].is_finite(),
                                    "non-finite position {n:?} in {}",
                                    p.display()
                                );
                                if snorm {
                                    assert!(
                                        (-1.0..=1.0).contains(&n[k]),
                                        "position {n:?} outside SNORM range in {}",
                                        p.display()
                                    );
                                } else {
                                    assert!(
                                        n[k] >= aabb.min[k] - margin
                                            && n[k] <= aabb.max[k] + margin,
                                        "float32 position {n:?} far outside AABB {aabb:?} in {}",
                                        p.display()
                                    );
                                }
                            }
                            if let Some(dequant) = comp.or((!snorm).then_some(aabb)) {
                                let w = sm.position_world(i, &dequant).unwrap();
                                for k in 0..3 {
                                    assert!(
                                        w[k] >= aabb.min[k] - margin
                                            && w[k] <= aabb.max[k] + margin,
                                        "world position {w:?} outside AABB {aabb:?} in {}",
                                        p.display()
                                    );
                                }
                            }
                            if snorm {
                                let raw = stms.position_raw(i).unwrap();
                                let base = i as usize * stride + off;
                                for (k, r) in raw.iter().enumerate() {
                                    assert_eq!(
                                        &stms.vertices[base + k * 2..base + k * 2 + 2],
                                        r.to_be_bytes(),
                                        "endianness mismatch at vert {i} in {}",
                                        p.display()
                                    );
                                }
                            }
                        }
                        checked += 1;
                    }
                }
            }
        }
        eprintln!("verified SNORM positions + endianness for {checked} submeshes");
        assert!(checked > 0, "no submeshes verified");
    }
}
