//! `SEDBmtb` motion: per-bone animation curves and per-material shader-constant curves.

use byteorder::{BigEndian as BE, ByteOrder, LittleEndian as LE};

const MIN_SUPPORTED_VERSION: u8 = 0x42;

/// Above this, SPU tracks identify a joint by [`joint_hash`] instead of skeleton index.
const MAX_JOINT_INDEX_VERSION: u8 = 0x45;

/// FNV-**1** (multiply then xor), not the more common FNV-1a.
pub fn joint_hash(name: &str) -> u32 {
    let h = name
        .bytes()
        .fold(2166136261u32, |h, b| h.wrapping_mul(16777619) ^ b as u32);
    h & 0x7fff_ffff
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Translation,
    Rotation,
}

struct Track {
    bone: usize,
    kind: Kind,
    axes: [Option<Vec<(u32, f32)>>; 3],
    /// The bone's ABSOLUTE local rotation, not a delta from bind.
    quat: Option<Vec<(u32, [f32; 4])>>,
}

#[derive(Clone, Copy, Debug)]
pub struct MaterialKey {
    pub value: f32,
    pub frame: u16,
    /// An angle in units of `2π/65536`, not a gradient; see [`MaterialKey::slope`].
    pub tangent: i16,
}

impl MaterialKey {
    pub fn slope(self) -> f32 {
        (self.tangent as f32 * std::f32::consts::PI / 65536.0).tan()
    }
}

/// A curve driving one shader constant, or one component of one, over the clip.
pub struct MaterialTrack {
    /// The name as stored, e.g. `uvofs0X`.
    pub constant: String,
    /// `constant` without its component letter, e.g. `uvofs0`.
    pub register: String,
    pub component: Option<u8>,
    /// 4 = linear, anything else = Hermite through the key tangents.
    pub kind: u16,
    pub keys: Vec<MaterialKey>,
    /// Set instead of `keys` when the curve is a single unchanging value.
    pub constant_value: Option<f32>,
}

impl MaterialTrack {
    /// Clamped to the key range.
    pub fn sample(&self, frame: f32) -> f32 {
        if let Some(v) = self.constant_value {
            return v;
        }
        let (Some(first), Some(last)) = (self.keys.first(), self.keys.last()) else {
            return 0.0;
        };
        if self.keys.len() == 1 || frame <= first.frame as f32 {
            return first.value;
        }
        if frame >= last.frame as f32 {
            return last.value;
        }
        let i = (0..self.keys.len() - 2)
            .take_while(|&i| (self.keys[i + 1].frame as f32) <= frame)
            .last()
            .map_or(0, |i| i + 1);
        let (a, b) = (self.keys[i], self.keys[i + 1]);
        let span = b.frame as f32 - a.frame as f32;
        if span <= 0.0 {
            return a.value;
        }
        let t = (frame - a.frame as f32) / span;
        if self.kind == 4 {
            return a.value + (b.value - a.value) * t;
        }
        let (m0, m1) = (a.slope() * span, b.slope() * span);
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * a.value
            + (t3 - 2.0 * t2 + t) * m0
            + (-2.0 * t3 + 3.0 * t2) * b.value
            + (t3 - t2) * m1
    }
}

pub struct MaterialAnim {
    pub material: String,
    pub tracks: Vec<MaterialTrack>,
}

impl MaterialAnim {
    pub fn overlay_at(&self, frame: f32) -> ConstantOverlay {
        let mut out: Vec<(String, [f32; 4], u8)> = Vec::new();
        for t in &self.tracks {
            let lane = t.component.unwrap_or(0).min(3) as usize;
            let v = t.sample(frame);
            match out.iter_mut().find(|(n, _, _)| *n == t.register) {
                Some((_, values, mask)) => {
                    values[lane] = v;
                    *mask |= 1 << lane;
                }
                None => {
                    let mut values = [0.0; 4];
                    values[lane] = v;
                    out.push((t.register.clone(), values, 1 << lane));
                }
            }
        }
        ConstantOverlay(out)
    }
}

/// One material's constants at one frame. Only lanes a curve actually drives are marked, so
/// [`ConstantOverlay::apply`] leaves the rest at whatever the shader resolved.
#[derive(Default)]
pub struct ConstantOverlay(Vec<(String, [f32; 4], u8)>);

impl ConstantOverlay {
    pub fn apply(&self, name: &str, resolved: &mut [f32; 4]) {
        let Some((_, values, mask)) = self.0.iter().find(|(n, _, _)| n == name) else {
            return;
        };
        for (lane, slot) in resolved.iter_mut().enumerate() {
            if mask & (1 << lane) != 0 {
                *slot = values[lane];
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(n, _, _)| n.as_str())
    }
}

pub struct Motion {
    pub fps: f32,
    pub frames: f32,
    tracks: Vec<Track>,
    /// `Track::bone` holds a [`joint_hash`], so the clip animates nothing until
    /// [`Motion::resolve_joints`] maps it onto a skeleton.
    bones_hashed: bool,
    /// Independent of `tracks`: a clip may carry only these, only skeletal tracks, or both.
    material: Vec<MaterialAnim>,
}

impl Motion {
    /// The flag is whether we can play the clip at all; unsupported and empty clips are still listed.
    pub fn clip_support(wpd_bytes: &[u8]) -> Vec<(String, bool)> {
        crate::wpd::Wpd::parse(wpd_bytes)
            .map(|w| {
                w.entries
                    .iter()
                    .filter(|e| e.ext == "mtb")
                    .map(|e| (e.name.clone(), playable(&e.data)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `name` is the clip's `.mtb` member name inside the pack.
    pub fn from_wpd_named(wpd_bytes: &[u8], name: &str) -> Option<Motion> {
        let wpd = crate::wpd::Wpd::parse(wpd_bytes).ok()?;
        let mtb = &wpd
            .entries
            .iter()
            .find(|e| e.ext == "mtb" && e.name == name)?
            .data;
        Motion::parse(mtb)
    }

    /// `None` for old-format clips and for clips carrying neither kind of animation.
    pub fn parse(mtb: &[u8]) -> Option<Motion> {
        if !is_supported(mtb) {
            return None;
        }
        let material = parse_material(mtb);
        match parse_spu(mtb) {
            Some(mut m) => {
                m.material = material;
                Some(m)
            }
            None if !material.is_empty() => {
                let (fps, frames) = clip_timing(mtb);
                Some(Motion {
                    fps,
                    frames,
                    tracks: Vec::new(),
                    bones_hashed: false,
                    material,
                })
            }
            None => None,
        }
    }

    pub fn material_anims(&self) -> &[MaterialAnim] {
        &self.material
    }

    pub fn has_skeletal(&self) -> bool {
        !self.tracks.is_empty()
    }

    pub fn needs_joint_resolve(&self) -> bool {
        self.bones_hashed
    }

    /// A no-op for XIII clips, whose tracks already carry indices. Tracks whose hash matches no
    /// joint are dropped; returns how many resolved.
    pub fn resolve_joints<S: AsRef<str>>(&mut self, joint_names: &[S]) -> usize {
        if !self.bones_hashed {
            return self.tracks.len();
        }
        let table: Vec<(u32, usize)> = joint_names
            .iter()
            .enumerate()
            .map(|(i, n)| (joint_hash(n.as_ref()), i))
            .collect();
        self.tracks.retain_mut(
            |t| match table.iter().find(|(h, _)| *h as usize == t.bone) {
                Some((_, idx)) => {
                    t.bone = *idx;
                    true
                }
                None => false,
            },
        );
        self.bones_hashed = false;
        self.tracks.len()
    }

    /// Layered on `bind`, which untracked bones get back unchanged. With `root_motion` false the
    /// root's translation is held at bind so the clip plays in place.
    pub fn local(
        &self,
        bone: usize,
        frame: f32,
        bind: [[f32; 4]; 4],
        root_motion: bool,
    ) -> [[f32; 4]; 4] {
        let Some(tr) = self.tracks.iter().find(|t| t.bone == bone) else {
            return bind;
        };
        let v = |a: usize, d: f32| tr.axes[a].as_ref().map(|k| sample(k, frame)).unwrap_or(d);
        let mut m = bind;
        if let Some(q) = &tr.quat {
            let r = rot3(sample_quat(q, frame));
            for c in 0..3 {
                m[c][..3].copy_from_slice(&r[c]);
            }
        }
        if tr.kind == Kind::Translation && root_motion {
            m[3][0] = v(0, bind[3][0]);
            m[3][1] = v(1, bind[3][1]);
            m[3][2] = v(2, bind[3][2]);
        }
        m
    }

    pub fn animated_bones(&self) -> usize {
        self.tracks.len()
    }
}

fn is_supported(mtb: &[u8]) -> bool {
    mtb.len() >= 0x34
        && mtb.starts_with(b"SEDBmtb")
        && mtb.get(8).copied().unwrap_or(0) >= MIN_SUPPORTED_VERSION
}

/// Runs the real decode so a caller's enabled/greyed state can never disagree with [`Motion::parse`].
fn playable(mtb: &[u8]) -> bool {
    Motion::parse(mtb).is_some()
}

/// Hyperspherical decode of a 48-bit key's low 43 bits, in direct order `(rx, ry, rz, w)`.
fn decode_q5(key: u64, hi_prec: bool) -> [f32; 4] {
    use std::f32::consts::FRAC_PI_2;
    let fa = (key & 0x1ffff) as f32 / 131071.0;
    let mut w = if hi_prec { 1.0 - fa * fa } else { fa };
    let s = (1.0 - w * w).max(0.0).sqrt();
    let b = ((key >> 17) & 0x3fffff) as u32;
    let row = (b as f32).sqrt().floor();
    let col = b as f32 - row * row;
    let phi = if row > 0.0 {
        FRAC_PI_2 * col / (2.0 * row)
    } else {
        0.0
    };
    let theta = FRAC_PI_2 * (1.0 - row / 2047.0);
    let (st, ct) = (theta.sin(), theta.cos());
    let (sp, cp) = (phi.sin(), phi.cos());
    let (rx, ry, rz) = (s * ct * cp, s * st, s * ct * sp);
    // Bits 39-42 are sign flags; bit 42 set means flip all four before applying the rest.
    let mut sf = ((key >> 39) & 0xf) as u32;
    if sf & 0x8 != 0 {
        sf ^= 0xf;
        w = -w;
    }
    let x = if sf & 0x4 != 0 { -rx } else { rx };
    let y = if sf & 0x2 != 0 { -ry } else { ry };
    let z = if sf & 0x1 != 0 { -rz } else { rz };
    [x, y, z, w]
}

fn sample(keys: &[(u32, f32)], frame: f32) -> f32 {
    match keys.first() {
        None => 0.0,
        Some(&(f0, v0)) if frame <= f0 as f32 => v0,
        _ => {
            for w in keys.windows(2) {
                if frame <= w[1].0 as f32 {
                    let (a, b) = (w[0], w[1]);
                    let t = (frame - a.0 as f32) / ((b.0 - a.0).max(1) as f32);
                    return a.1 + (b.1 - a.1) * t;
                }
            }
            keys.last().unwrap().1
        }
    }
}

/// Shortest-path nlerp, clamped to the ends.
fn sample_quat(keys: &[(u32, [f32; 4])], frame: f32) -> [f32; 4] {
    let pick = match keys.first() {
        None => return [0.0, 0.0, 0.0, 1.0],
        Some(&(f0, v0)) if frame <= f0 as f32 => v0,
        _ => {
            let mut chosen = keys.last().unwrap().1;
            for w in keys.windows(2) {
                if frame <= w[1].0 as f32 {
                    let (a, mut b) = (w[0], w[1]);
                    let t = (frame - a.0 as f32) / ((b.0 - a.0).max(1) as f32);
                    if (0..4).map(|i| a.1[i] * b.1[i]).sum::<f32>() < 0.0 {
                        b.1 = b.1.map(|x| -x);
                    }
                    chosen = std::array::from_fn(|i| a.1[i] + (b.1[i] - a.1[i]) * t);
                    break;
                }
            }
            chosen
        }
    };
    let n = pick.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    pick.map(|x| x / n)
}

/// Returns the offset of the track's channel data, resolved through the name-indexed track table.
fn find_track(mtb: &[u8], name: &str) -> Option<usize> {
    let tt = TrackTable::read(mtb)?;
    let name_idx = (0..tt.name_count).find(|&i| tt.track_name(i) == Some(name))?;
    (0..tt.count).find_map(|k| {
        let eo = tt.entry(k)?;
        (LE::read_u32(mtb.get(eo..eo + 4)?) as usize == name_idx).then_some(eo + 0x10)
    })
}

/// Entry offsets plus two string pools: track names, and the `chunk` pool material sections name
/// their material and constants from.
struct TrackTable<'a> {
    mtb: &'a [u8],
    entries: usize,
    count: usize,
    names: usize,
    name_count: usize,
    chunks: usize,
    chunk_count: usize,
}

impl<'a> TrackTable<'a> {
    fn read(mtb: &'a [u8]) -> Option<TrackTable<'a>> {
        let u32 = |o: usize| mtb.get(o..o + 4).map(LE::read_u32);
        let hdr = mtb.get(0xe..0x10).map(LE::read_u16)? as usize;
        let count = u32(hdr)? as usize;
        let entries = hdr + 4;
        let name_count = u32(entries + count * 4)? as usize;
        let names = entries + count * 4 + 4;
        let chunk_count = u32(names + name_count * 4)? as usize;
        let chunks = names + name_count * 4 + 4;
        (chunk_count < 4096).then_some(TrackTable {
            mtb,
            entries,
            count,
            names,
            name_count,
            chunks,
            chunk_count,
        })
    }

    fn entry(&self, i: usize) -> Option<usize> {
        self.mtb
            .get(self.entries + i * 4..self.entries + i * 4 + 4)
            .map(|b| LE::read_u32(b) as usize)
    }

    fn cstr(&self, at: usize) -> Option<&'a str> {
        let end = self.mtb.get(at..)?.iter().position(|&b| b == 0)? + at;
        std::str::from_utf8(&self.mtb[at..end]).ok()
    }

    fn name_from(&self, table: usize, limit: usize, i: usize) -> Option<&'a str> {
        if i >= limit {
            return None;
        }
        let off = self
            .mtb
            .get(table + i * 4..table + i * 4 + 4)
            .map(LE::read_u32)? as usize;
        self.cstr(off)
    }

    fn track_name(&self, i: usize) -> Option<&'a str> {
        self.name_from(self.names, self.name_count, i)
    }

    fn chunk_name(&self, i: usize) -> Option<&'a str> {
        self.name_from(self.chunks, self.chunk_count, i)
    }
}

fn parse_spu(mtb: &[u8]) -> Option<Motion> {
    use std::collections::BTreeMap;
    let spu = find_track(mtb, "SpuBinary")?;
    let hashed = mtb.get(8).copied().unwrap_or(0) > MAX_JOINT_INDEX_VERSION;
    let be32 = |o: usize| mtb.get(o..o + 4).map(BE::read_u32);
    let be16 = |o: usize| mtb.get(o..o + 2).map(BE::read_u16);
    let base = spu + 4 + 12;
    // NumSections is a byte plus a pad byte, not a u16: XIII zeroes the pad but XIII-2 puts 0x01 there.
    let nsec = *mtb.get(base)? as usize;
    let sect = base + 4 + 12;
    type Curves = (Option<Vec<(u32, [f32; 4])>>, [Option<Vec<(u32, f32)>>; 3]);
    let mut by_bone: BTreeMap<usize, Curves> = BTreeMap::new();
    for si in 0..nsec {
        let so = sect + si * 8;
        let secoff = be32(so)? as usize;
        let nchunks = be32(so + 4)? as usize;
        let ctab = base + secoff;
        for ci in 0..nchunks {
            let co = ctab + ci * 8;
            let choff = be32(co)? as usize;
            let nchild = *mtb.get(co + 6)? as usize + 1;
            let mut p = base + choff;
            for _ in 0..nchild {
                // Still an 8-byte entry, but the widened joint field pushes type and count back two.
                let (bone, ty, count) = if hashed {
                    (be32(p)? as usize, be16(p + 4)? as usize, be16(p + 6)?)
                } else {
                    (be16(p)? as usize, be16(p + 2)? as usize, be16(p + 4)?)
                };
                p += 8;
                match ty {
                    0 => {
                        let (k, np) = parse_spu_quat(mtb, p, count)?;
                        by_bone.entry(bone).or_default().0 = Some(k);
                        p = np;
                    }
                    4..=9 => {
                        let (k, np) = parse_spu_linear(mtb, p, count)?;
                        if (4..=6).contains(&ty) {
                            by_bone.entry(bone).or_default().1[ty - 4] = Some(k);
                        }
                        p = np;
                    }
                    0xb..=0x10 => p += 4,
                    0x11..=0x13 => p += 6 * count as usize,
                    _ => return None,
                }
            }
        }
    }
    let (fps, frames) = clip_timing(mtb);
    let tracks: Vec<Track> = by_bone
        .into_iter()
        .map(|(bone, (quat, axes))| {
            let kind = if axes.iter().any(|a| a.is_some()) {
                Kind::Translation
            } else {
                Kind::Rotation
            };
            Track {
                bone,
                kind,
                axes,
                quat,
            }
        })
        .collect();
    (!tracks.is_empty()).then_some(Motion {
        fps,
        frames,
        tracks,
        bones_hashed: hashed,
        material: Vec::new(),
    })
}

/// A decoded curve plus the read cursor after it.
type Curve<T> = (Vec<(u32, T)>, usize);

/// `count` bit 7 is a precision flag, not part of the segment count.
fn parse_spu_quat(d: &[u8], o: usize, count: u16) -> Option<Curve<[f32; 4]>> {
    let flag = count & 0x80 != 0;
    let n = (count & 0xff7f) as usize;
    let lengths: Vec<u8> = (0..n)
        .map(|i| d.get(o + i).copied())
        .collect::<Option<_>>()?;
    let mut p = o + ((n + 3) & !3);
    let mut keys = Vec::new();
    for (i, &len) in lengths.iter().enumerate() {
        for _ in 0..len {
            let key = (BE::read_u16(d.get(p..p + 2)?) as u64) << 32
                | (BE::read_u16(d.get(p + 2..p + 4)?) as u64) << 16
                | BE::read_u16(d.get(p + 4..p + 6)?) as u64;
            p += 6;
            let time = ((key >> 43) & 0x1f) as u32 + 32 * i as u32;
            keys.push((time, decode_q5(key, flag)));
        }
    }
    Some((keys, p))
}

/// Defaults to 30fps and one frame when the `Header` track is missing.
fn clip_timing(mtb: &[u8]) -> (f32, f32) {
    let (mut fps, mut frames) = (30.0, 1.0);
    if let Some(h) = find_track(mtb, "Header")
        && let (Some(a), Some(b)) = (mtb.get(h..h + 4), mtb.get(h + 4..h + 8))
    {
        let (f, fr) = (LE::read_f32(a), LE::read_f32(b));
        if (1.0..=240.0).contains(&f) {
            fps = f;
        }
        if (1.0..=1.0e6).contains(&fr) {
            frames = fr;
        }
    }
    (fps, frames)
}

/// The preceding character must be lower-case, so all-caps names like `MATRIXW` stay whole.
fn split_component(name: &str) -> (&str, Option<u8>) {
    let mut it = name.chars().rev();
    let comp = match it.next() {
        Some('X') => 0,
        Some('Y') => 1,
        Some('Z') => 2,
        Some('W') => 3,
        _ => return (name, None),
    };
    match it.next() {
        Some(p) if !p.is_uppercase() => (&name[..name.len() - 1], Some(comp)),
        _ => (name, None),
    }
}

/// Every track other than `Header`, `SpuBinary` and the skeletal `root`/`face` groups is a material.
fn parse_material(mtb: &[u8]) -> Vec<MaterialAnim> {
    let Some(tt) = TrackTable::read(mtb) else {
        return Vec::new();
    };
    let mut starts: Vec<usize> = (0..tt.count).filter_map(|k| tt.entry(k)).collect();
    starts.sort_unstable();
    let mut out = Vec::new();
    for k in 0..tt.count {
        let Some(base) = tt.entry(k) else { continue };
        let name = mtb
            .get(base..base + 4)
            .map(LE::read_u32)
            .and_then(|i| tt.track_name(i as usize));
        if matches!(name, None | Some("Header" | "SpuBinary" | "root" | "face")) {
            continue;
        }
        let end = starts
            .iter()
            .copied()
            .find(|&s| s > base)
            .unwrap_or(mtb.len());
        if let Some(anim) = parse_material_section(mtb, &tt, base, end)
            && !anim.tracks.is_empty()
        {
            out.push(anim);
        }
    }
    out
}

fn parse_material_section(
    mtb: &[u8],
    tt: &TrackTable,
    base: usize,
    end: usize,
) -> Option<MaterialAnim> {
    let u32 = |o: usize| {
        (o + 4 <= end)
            .then(|| mtb.get(o..o + 4).map(LE::read_u32))
            .flatten()
    };
    let head = base + 16;
    let material = tt
        .chunk_name((u32(head)? & 0x7fff_ffff) as usize)
        .unwrap_or_default()
        .to_string();
    let count = u32(head + 4)?;
    // Each track costs at least 12 bytes, so a count past that many is a misread section.
    if count as usize > (end - head).saturating_sub(8) / 12 {
        return None;
    }
    let mut at = head + 8;
    let mut tracks = Vec::new();
    for _ in 0..count {
        let constant = tt
            .chunk_name((u32(at)? & 0x7fff_ffff) as usize)
            .unwrap_or_default()
            .to_string();
        let packed = u32(at + 4)?;
        let (register, component) = split_component(&constant);
        let (register, component) = (register.to_string(), component);
        if packed == 0 {
            let value = f32::from_bits(u32(at + 8)?);
            tracks.push(MaterialTrack {
                constant,
                register,
                component,
                kind: 0,
                keys: Vec::new(),
                constant_value: Some(value),
            });
            at += 12;
            continue;
        }
        let key_count = (packed >> 16) as usize;
        let kind = (packed & 0xffff) as u16;
        if key_count == 0 || at + 8 + key_count * 8 > end {
            break;
        }
        let keys = (0..key_count)
            .map(|i| {
                let o = at + 8 + i * 8;
                MaterialKey {
                    value: f32::from_bits(LE::read_u32(&mtb[o..o + 4])),
                    frame: LE::read_u16(&mtb[o + 4..o + 6]),
                    tangent: LE::read_u16(&mtb[o + 6..o + 8]) as i16,
                }
            })
            .collect();
        tracks.push(MaterialTrack {
            constant,
            register,
            component,
            kind,
            keys,
            constant_value: None,
        });
        at += 8 + key_count * 8;
    }
    Some(MaterialAnim { material, tracks })
}

/// `count` bit 15 selects 16-bit over 8-bit values; the low 15 bits are the segment count.
fn parse_spu_linear(d: &[u8], o: usize, count: u16) -> Option<Curve<f32>> {
    let wide = count & 0x8000 != 0;
    let n = (count & 0x7fff) as usize;
    let offset = BE::read_f32(d.get(o..o + 4)?);
    let scale = BE::read_f32(d.get(o + 4..o + 8)?);
    let lengths: Vec<u8> = (0..n)
        .map(|i| d.get(o + 8 + i).copied())
        .collect::<Option<_>>()?;
    let mut p = o + 8 + n;
    let mut keys = Vec::new();
    for (i, &lraw) in lengths.iter().enumerate() {
        let len = if lraw == 0 { 1 } else { lraw } as usize;
        let indices: Vec<u8> = (0..len)
            .map(|j| d.get(p + j).copied())
            .collect::<Option<_>>()?;
        p += len;
        for &idx in &indices {
            let val = if wide {
                let sv = BE::read_u16(d.get(p..p + 2)?);
                p += 2;
                let mag = (sv & 0x7fff) as f32 / 32767.0;
                (if sv & 0x8000 != 0 { -mag } else { mag }) * scale + offset
            } else {
                let sv = *d.get(p)?;
                p += 1;
                let mag = (sv & 0x7f) as f32 / 127.0;
                (if sv & 0x80 != 0 { -mag } else { mag }) * scale + offset
            };
            keys.push((idx as u32 + 256 * i as u32, val));
        }
    }
    Some((keys, (p + 3) & !3))
}

/// Column-major.
fn rot3(q: [f32; 4]) -> [[f32; 3]; 3] {
    let [x, y, z, w] = q;
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + z * w),
            2.0 * (x * z - y * w),
        ],
        [
            2.0 * (x * y - z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + x * w),
        ],
        [
            2.0 * (x * z + y * w),
            2.0 * (y * z - x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_suffix_splits_only_after_lowercase() {
        assert_eq!(split_component("uvofs1X"), ("uvofs1", Some(0)));
        assert_eq!(split_component("diffuseScaleZ"), ("diffuseScale", Some(2)));
        assert_eq!(
            split_component("multiDiffuseColorW"),
            ("multiDiffuseColor", Some(3))
        );
        assert_eq!(split_component("shininess"), ("shininess", None));
        assert_eq!(split_component("MATRIXW"), ("MATRIXW", None));
        assert_eq!(split_component("X"), ("X", None));
    }

    #[test]
    fn linear_curve_interpolates_between_keys_and_clamps() {
        let t = MaterialTrack {
            constant: "uvofs1X".into(),
            register: "uvofs1".into(),
            component: Some(0),
            kind: 4,
            keys: vec![
                MaterialKey {
                    value: 0.0,
                    frame: 0,
                    tangent: 0,
                },
                MaterialKey {
                    value: 1.0,
                    frame: 100,
                    tangent: 0,
                },
            ],
            constant_value: None,
        };
        assert_eq!(t.sample(-5.0), 0.0);
        assert!((t.sample(25.0) - 0.25).abs() < 1e-6);
        assert!((t.sample(50.0) - 0.5).abs() < 1e-6);
        assert_eq!(t.sample(1000.0), 1.0);
    }

    #[test]
    fn hermite_curve_passes_through_its_keys() {
        let t = MaterialTrack {
            constant: "diffuseScaleX".into(),
            register: "diffuseScale".into(),
            component: Some(0),
            kind: 3,
            keys: vec![
                MaterialKey {
                    value: 2.0,
                    frame: 0,
                    tangent: 0,
                },
                MaterialKey {
                    value: 5.0,
                    frame: 10,
                    tangent: 1000,
                },
                MaterialKey {
                    value: 1.0,
                    frame: 20,
                    tangent: 0,
                },
            ],
            constant_value: None,
        };
        for k in &t.keys {
            assert!(
                (t.sample(k.frame as f32) - k.value).abs() < 1e-4,
                "key at {} not hit",
                k.frame
            );
        }
    }

    #[test]
    fn constant_track_ignores_frame() {
        let t = MaterialTrack {
            constant: "shininess".into(),
            register: "shininess".into(),
            component: None,
            kind: 0,
            keys: Vec::new(),
            constant_value: Some(4.0),
        };
        assert_eq!(t.sample(0.0), 4.0);
        assert_eq!(t.sample(999.0), 4.0);
    }

    #[test]
    fn shipped_uv_clip_decodes() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let p = format!("{dir}/mot/weapon/sk_w661_pgolem/t1.white.win32.bin");
        let Ok(b) = std::fs::read(&p) else { return };
        let m = Motion::from_wpd_named(&b, "t1w661UV00_01").expect("clip decodes");
        assert!(!m.has_skeletal(), "UV clip should carry no skeletal tracks");
        let anims = m.material_anims();
        assert_eq!(anims.len(), 1);
        assert_eq!(anims[0].material, "w661_all");
        let t = &anims[0].tracks[0];
        assert_eq!(t.register, "uvofs1");
        assert_eq!(t.component, Some(0));
        assert_eq!(t.kind, 4);
        assert!((t.sample(0.0) - 0.0).abs() < 1e-6);
        assert!((t.sample(m.frames) - 1.0).abs() < 1e-6);
        assert!((t.sample(m.frames / 2.0) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn shipped_clip_mixes_skeletal_and_material() {
        let Ok(dir) = std::env::var("FF13_GAME_DIR") else {
            return;
        };
        let p = format!("{dir}/mot/npc/sk_n916_rgf/t1.white.win32.bin");
        let Ok(b) = std::fs::read(&p) else { return };
        let m = Motion::from_wpd_named(&b, "t1n916UV_53").expect("clip decodes");
        assert!(m.has_skeletal());
        let anims = m.material_anims();
        assert_eq!(anims.len(), 2);
        for a in anims {
            assert!(!a.material.is_empty());
            for t in &a.tracks {
                assert!(!t.register.is_empty());
                for k in &t.keys {
                    assert!(
                        k.frame as f32 <= m.frames,
                        "{} key {} past clip end {}",
                        t.constant,
                        k.frame,
                        m.frames
                    );
                }
            }
        }
    }

    #[test]
    fn overlay_merges_components_and_leaves_undriven_lanes_alone() {
        let mk = |name: &str, v: f32| {
            let (register, component) = split_component(name);
            MaterialTrack {
                constant: name.into(),
                register: register.into(),
                component,
                kind: 0,
                keys: Vec::new(),
                constant_value: Some(v),
            }
        };
        let anim = MaterialAnim {
            material: "m".into(),
            tracks: vec![
                mk("reflectivityX", 0.5),
                mk("reflectivityZ", 0.25),
                mk("shininess", 4.0),
            ],
        };
        let ov = anim.overlay_at(0.0);
        let mut c = [9.0, 9.0, 9.0, 9.0];
        ov.apply("reflectivity", &mut c);
        assert_eq!(c, [0.5, 9.0, 0.25, 9.0], "undriven lanes must survive");
        let mut s = [9.0, 9.0, 9.0, 9.0];
        ov.apply("shininess", &mut s);
        assert_eq!(
            s,
            [4.0, 9.0, 9.0, 9.0],
            "component-less curve drives lane 0"
        );
        let mut untouched = [7.0; 4];
        ov.apply("diffuseColor", &mut untouched);
        assert_eq!(untouched, [7.0; 4], "unknown constant is left alone");
    }
}
