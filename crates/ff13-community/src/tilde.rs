//! The `:~` mesh-import op: adaptive edge-split subdivision of an existing submesh.

use std::collections::HashMap;

use super::modbundle::ControlVertex;
use byteorder::{BigEndian as BE, ByteOrder};
use ff13_formats::wrb::Chunk;

/// Truncates rather than rounds.
pub fn encode_pos(p: [f32; 3]) -> [i16; 3] {
    p.map(|c| (c.clamp(-1.0, 1.0) * 32767.0) as i16)
}

/// An `:e` selection region gating which subdivision verts a later `:~` keeps. Despite the name
/// "region deformation", it is a pure containment predicate, not a displacement.
#[derive(Clone, Copy, Debug)]
pub struct Region {
    /// `mode % 100`: `<0` exclusion, `0`/`1` inclusion box, `>=2` inclusion ellipsoid.
    pub ty: i16,
    pub half: [f32; 3],
    /// Euler angles in radians, stored negated.
    pub rot: [f32; 3],
    pub center: [f32; 3],
}

impl Region {
    /// Only the 9-float form; the 200/201 min/max-corner forms are unimplemented. A `true` `reset`
    /// means the region replaces the list rather than appending to it.
    pub fn parse(args: &[String]) -> Option<(Region, bool)> {
        let mode: i32 = args.first()?.parse().ok()?;
        let f: Vec<f32> = args[1..].iter().filter_map(|s| s.parse().ok()).collect();
        if f.len() < 9 {
            return None;
        }
        Some((
            Region {
                ty: (mode % 100) as i16,
                half: [f[0] * 0.5, f[1] * 0.5, f[2] * 0.5],
                rot: [-f[3], -f[4], -f[5]],
                center: [f[6], f[7], f[8]],
            },
            mode.abs() <= 99,
        ))
    }

    /// Inverse-rotates, since the stored angles are applied directly.
    fn to_local(self, p: [f32; 3]) -> [f32; 3] {
        let rot2d = |a: f32, b: f32, t: f32| -> (f32, f32) {
            if (a == 0.0 && b == 0.0) || t == 0.0 {
                return (a, b);
            }
            let r = (a * a + b * b).sqrt();
            let ang = b.atan2(a) + t;
            (ang.sin() * r, ang.cos() * r)
        };
        let mut l = [
            p[0] - self.center[0],
            p[1] - self.center[1],
            p[2] - self.center[2],
        ];
        let (y, z) = rot2d(l[1], l[2], self.rot[0]);
        l[1] = y;
        l[2] = z;
        let (x, z) = rot2d(l[0], l[2], self.rot[1]);
        l[0] = x;
        l[2] = z;
        let (x, y) = rot2d(l[0], l[1], self.rot[2]);
        l[0] = x;
        l[1] = y;
        l
    }
}

/// Exclusions win over inclusions, and a point matching nothing is dropped.
fn region_contains(p: [f32; 3], regions: &[Region]) -> bool {
    let in_box = |l: [f32; 3], r: &Region| {
        l[0].abs() <= r.half[0] && l[1].abs() <= r.half[1] && l[2].abs() <= r.half[2]
    };
    let ellip = |l: [f32; 3], r: &Region, z: bool| {
        let mut e = l[0] * l[0] / (r.half[0] * r.half[0]) + l[1] * l[1] / (r.half[1] * r.half[1]);
        if z {
            e += l[2] * l[2] / (r.half[2] * r.half[2]);
        }
        e
    };
    for r in regions.iter().filter(|r| r.ty < 0) {
        let l = r.to_local(p);
        let inside = if r.ty == -1 {
            in_box(l, r)
        } else {
            ellip(l, r, false) <= 1.0 && l[2].abs() <= r.half[2]
        };
        if inside {
            return false;
        }
    }
    for r in regions.iter().filter(|r| r.ty >= 0) {
        let l = r.to_local(p);
        let inside = if r.ty <= 1 {
            in_box(l, r)
        } else {
            ellip(l, r, true) <= 1.0
        };
        if inside {
            return true;
        }
    }
    false
}

/// New verts to append, positions only, plus the new triangle list. Index `orig_vert_count + k`
/// refers to `new_positions[k]`; anything below that is an original vertex.
#[derive(Debug, Default)]
pub struct Subdivision {
    pub new_positions: Vec<[i16; 3]>,
    /// The complete output buffer, not just the split pieces.
    pub indices: Vec<[u32; 3]>,
    /// The split edge's two endpoints, which callers interpolate normals, UVs and skin from.
    pub new_vert_edge: Vec<(u32, u32)>,
    /// The bundle carries corner controls too, and each affected corner is rewritten to its morph.
    /// Keyed old raw to new raw, so seam duplicates at the same position move together.
    pub corner_overrides: HashMap<[i16; 3], [i16; 3]>,
}

fn d2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}
fn mid(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        (a[0] + b[0]) / 2.0,
        (a[1] + b[1]) / 2.0,
        (a[2] + b[2]) / 2.0,
    ]
}

/// Cells exceed the query radius, so the 3x3x3 neighborhood holds every in-range point.
struct Grid {
    cell: f32,
    map: HashMap<[i32; 3], Vec<usize>>,
}

impl Grid {
    fn new(radius: f32) -> Grid {
        Grid {
            cell: radius * 1.01,
            map: HashMap::new(),
        }
    }

    fn key(&self, p: [f32; 3]) -> [i32; 3] {
        p.map(|c| (c / self.cell).floor() as i32)
    }

    fn insert(&mut self, p: [f32; 3], id: usize) {
        self.map.entry(self.key(p)).or_default().push(id);
    }

    fn for_each_near(&self, p: [f32; 3], mut f: impl FnMut(usize)) {
        let k = self.key(p);
        for dx in -1i32..=1 {
            for dy in -1i32..=1 {
                for dz in -1i32..=1 {
                    let cell = [
                        k[0].saturating_add(dx),
                        k[1].saturating_add(dy),
                        k[2].saturating_add(dz),
                    ];
                    for &id in self.map.get(&cell).into_iter().flatten() {
                        f(id);
                    }
                }
            }
        }
    }
}

/// `orig_pos` is model-space (`raw/32767`), `controls` are the entry's `(pos, morph)` pairs, and
/// `merge_eps` is the snap threshold.
pub fn subdivide(
    orig_pos: &[[f32; 3]],
    orig_raw: &[[i16; 3]],
    orig_idx: &[u16],
    controls: &[ControlVertex],
    merge_eps: f32,
    region: Option<&[Region]>,
    reuse_src: &[(usize, usize, usize)],
) -> Subdivision {
    let nold = orig_pos.len();
    let mut out = Subdivision::default();
    let l1 =
        |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs();

    // Weld by L1 distance, not exact int16 equality, so near-coincident seam verts collapse.
    const WELD_EPS: f32 = 0.00015;
    let mut canon_pos: Vec<[f32; 3]> = Vec::new();
    let mut canon_grid = Grid::new(WELD_EPS);
    let mut cid: Vec<usize> = Vec::with_capacity(nold);
    for &p in orig_pos.iter().take(nold) {
        let mut first: Option<usize> = None;
        canon_grid.for_each_near(p, |i| {
            if l1(p, canon_pos[i]) < WELD_EPS && first.is_none_or(|f| i < f) {
                first = Some(i);
            }
        });
        match first {
            Some(i) => cid.push(i),
            None => {
                cid.push(canon_pos.len());
                canon_grid.insert(p, canon_pos.len());
                canon_pos.push(p);
            }
        }
    }
    // Splitting an edge a prior `:T` already added a vert for reuses that vert. Keying the dedup
    // on the endpoints is what makes it survive `:V` moves.
    let mut reuse_map: HashMap<(usize, usize), u32> = HashMap::new();
    for &(sa, sb, vidx) in reuse_src {
        if let (Some(&ca), Some(&cb)) = (cid.get(sa), cid.get(sb))
            && ca != cb
        {
            reuse_map.insert((ca.min(cb), ca.max(cb)), vidx as u32);
        }
    }

    // Index-buffer first-appearance order, which is deterministic; a HashMap's random order made
    // the nearest-edge tie-break non-reproducible.
    let mut edge_mids: Vec<((usize, usize), [f32; 3])> = Vec::new();
    let mut edge_seen: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    for t in orig_idx.chunks_exact(3) {
        for (i, j) in [(0, 1), (1, 2), (2, 0)] {
            let (a, b) = (cid[t[i] as usize], cid[t[j] as usize]);
            if a == b {
                continue;
            }
            let k = (a.min(b), a.max(b));
            if edge_seen.insert(k) {
                edge_mids.push((k, mid(canon_pos[a], canon_pos[b])));
            }
        }
    }

    // A control is an edge-split candidate iff it coincides with an existing edge midpoint, an
    // approximation of the real vertex-marking rule that can differ by about one vert on dense meshes.
    let drop_eps = merge_eps / 2.0;
    // An edge only splits when BOTH endpoints are marked corners. This is a no-op on simple meshes,
    // but on `:V`-moved or compound ones it drops edge matches that were never marked.
    let mut control_grid = Grid::new(WELD_EPS);
    for (i, c) in controls.iter().enumerate() {
        control_grid.insert(c.pos, i);
    }
    let corner: Vec<bool> = canon_pos
        .iter()
        .map(|cp| {
            let mut hit = false;
            control_grid.for_each_near(*cp, |i| hit = hit || l1(controls[i].pos, *cp) < 0.00015);
            hit
        })
        .collect();
    // First in bundle order, not nearest, is how a multi-match resolves.
    let first_control_within = |p: [f32; 3], eps: f32| -> Option<usize> {
        let mut first: Option<usize> = None;
        control_grid.for_each_near(p, |i| {
            if l1(controls[i].pos, p) < eps && first.is_none_or(|f| i < f) {
                first = Some(i);
            }
        });
        first
    };
    let mut mid_grid = Grid::new(0.0015);
    for (i, &(_, m)) in edge_mids.iter().enumerate() {
        mid_grid.insert(m, i);
    }
    let mut split: HashMap<(usize, usize), [f32; 3]> = HashMap::new();
    let mut best_dm: HashMap<(usize, usize), f32> = HashMap::new();
    let mut mid_cand: Vec<usize> = Vec::new();
    for c in controls {
        // Sorted ids plus a strict `<` reproduce the linear scan's first-minimum tie-break.
        mid_cand.clear();
        mid_grid.for_each_near(c.pos, |i| mid_cand.push(i));
        mid_cand.sort_unstable();
        let mut bd = f32::INFINITY;
        let mut be = None;
        let mut bm = [0f32; 3];
        for &i in &mid_cand {
            let (k, m) = edge_mids[i];
            let dd = d2(c.pos, m);
            if dd < bd {
                bd = dd;
                be = Some(k);
                bm = m;
            }
        }
        let dm = bd.sqrt();
        let Some(be) = be else {
            continue;
        };
        // A region set by `:e` is not cleared by `:E`, and gates corners by vertex.
        let region_ok = |p: [f32; 3]| region.is_none_or(|rg| region_contains(p, rg));
        if dm >= 0.0015 {
            continue;
        }
        let dend = d2(c.morph, canon_pos[be.0])
            .sqrt()
            .min(d2(c.morph, canon_pos[be.1]).sqrt());
        if dend < drop_eps {
            continue;
        }
        if d2(canon_pos[be.0], canon_pos[be.1]).sqrt() < merge_eps {
            continue;
        }
        if !(corner[be.0] && corner[be.1]) {
            continue;
        }
        if best_dm.get(&be).is_some_and(|&prev| prev <= dm) {
            continue;
        }
        // The vert starts at the linear midpoint and only moves to a morph if the midpoint matches
        // a bundle `pos`, taking the first such match rather than the nearest.
        if !region_ok(bm) {
            continue;
        }
        // The morph pass matches the already-quantized position, so quantize before the L1 test.
        let bmq = {
            let e = encode_pos(bm);
            [
                e[0] as f32 / 32767.0,
                e[1] as f32 / 32767.0,
                e[2] as f32 / 32767.0,
            ]
        };
        let final_pos = first_control_within(bmq, 0.0001).map_or(bm, |i| controls[i].morph);
        best_dm.insert(be, dm);
        split.insert(be, final_pos);
    }

    // Iterate verts rather than controls, so a multi-match vert resolves the same way and one
    // control still covers every vert it touches.
    for v in 0..nold {
        if !region.is_none_or(|rg| region_contains(orig_pos[v], rg)) {
            continue;
        }
        // The morph pass tolerance is tighter than corner marking's, and picks first not nearest.
        if let Some(i) = first_control_within(orig_pos[v], 0.0001) {
            out.corner_overrides
                .entry(orig_raw[v])
                .or_insert(encode_pos(controls[i].morph));
        }
    }

    // At `merge_eps == 0`, corner-pairs carrying no midpoint control split too, which is what makes
    // cumulative piercings work. Gated off above 0, where it would over-split.
    {
        let drop_grid = (merge_eps > 0.0).then(|| {
            let mut g = Grid::new(drop_eps);
            for (i, cp) in canon_pos.iter().enumerate() {
                g.insert(*cp, i);
            }
            g
        });
        for (k, m) in &edge_mids {
            if split.contains_key(k) || !(corner[k.0] && corner[k.1]) {
                continue;
            }
            if !region.is_none_or(|rg| region_contains(*m, rg)) {
                continue;
            }
            // A corner-pair midpoint within `drop_eps` of a vertex reuses that vertex instead.
            if let Some(g) = &drop_grid {
                let mut near = false;
                g.for_each_near(*m, |i| {
                    near = near || d2(*m, canon_pos[i]).sqrt() < drop_eps
                });
                if near {
                    continue;
                }
            }
            let mq = {
                let e = encode_pos(*m);
                [
                    e[0] as f32 / 32767.0,
                    e[1] as f32 / 32767.0,
                    e[2] as f32 / 32767.0,
                ]
            };
            let fp = first_control_within(mq, 0.0001).map_or(*m, |i| controls[i].morph);
            split.insert(*k, fp);
        }
    }

    // Dedup is per index-edge, not per geometric edge: an edge shared by two tris emits one
    // midpoint, but the same edge under seam-duplicated indices emits a fresh one each time.
    let nold_u = nold as u32;
    let mut idx_mid: HashMap<(u16, u16), u32> = HashMap::new();
    type Tri = ([u32; 3], [Option<u32>; 3]);
    let mut tris: Vec<Tri> = Vec::with_capacity(orig_idx.len() / 3);
    for t in orig_idx.chunks_exact(3) {
        let (a, b, c) = (cid[t[0] as usize], cid[t[1] as usize], cid[t[2] as usize]);
        let mut mid =
            |ti: u16, tj: u16, wi: usize, wj: usize, out: &mut Subdivision| -> Option<u32> {
                if wi == wj {
                    return None;
                }
                let wkey = (wi.min(wj), wi.max(wj));
                let morph = *split.get(&wkey)?;
                if let Some(&vidx) = reuse_map.get(&wkey) {
                    return Some(vidx);
                }
                let ikey = (ti.min(tj), ti.max(tj));
                if let Some(&id) = idx_mid.get(&ikey) {
                    return Some(nold_u + id);
                }
                let id = out.new_positions.len() as u32;
                out.new_positions.push(encode_pos(morph));
                out.new_vert_edge.push((ti as u32, tj as u32));
                idx_mid.insert(ikey, id);
                Some(nold_u + id)
            };
        // Winding order, except that splitting only 01 and 20 emits 20's midpoint first.
        let is_split = |wi: usize, wj: usize| split.contains_key(&(wi.min(wj), wi.max(wj)));
        let (sab, sbc, sca);
        if is_split(a, b) && is_split(c, a) && !is_split(b, c) {
            sca = mid(t[2], t[0], c, a, &mut out);
            sab = mid(t[0], t[1], a, b, &mut out);
            sbc = None;
        } else {
            sab = mid(t[0], t[1], a, b, &mut out);
            sbc = mid(t[1], t[2], b, c, &mut out);
            sca = mid(t[2], t[0], c, a, &mut out);
        }
        tris.push(([t[0] as u32, t[1] as u32, t[2] as u32], [sab, sbc, sca]));
    }

    // Each original tri keeps its slot, holding either itself or its split's anchor piece, with the
    // remaining pieces appended. Diagonals are chosen on the quantized un-morphed split, because
    // triangulation happens on the int16 linear midpoints, before any morph displaces them.
    let fpos = |v: u32| -> [i16; 3] {
        if (v as usize) < nold {
            return orig_raw[v as usize];
        }
        let (ti, tj) = out.new_vert_edge[(v as usize) - nold];
        let (pa, pb) = (orig_pos[ti as usize], orig_pos[tj as usize]);
        encode_pos([
            (pa[0] + pb[0]) * 0.5,
            (pa[1] + pb[1]) * 0.5,
            (pa[2] + pb[2]) * 0.5,
        ])
    };
    let dist2 = |x: u32, y: u32| {
        let (rx, ry) = (fpos(x), fpos(y));
        (0..3)
            .map(|k| (rx[k] as f64 - ry[k] as f64).powi(2))
            .sum::<f64>()
    };
    // Each appended quad tri is rotated to start with its first new vertex.
    let rot_new = |t: [u32; 3]| -> [u32; 3] {
        let p = (0..3).find(|&i| (t[i] as usize) >= nold).unwrap_or(0);
        [t[p], t[(p + 1) % 3], t[(p + 2) % 3]]
    };
    let rot_to = |v: u32, t: [u32; 3]| -> [u32; 3] {
        let p = (0..3).find(|&i| t[i] == v).unwrap_or(0);
        [t[p], t[(p + 1) % 3], t[(p + 2) % 3]]
    };
    // The two-midpoint tri starts at the first split edge in original winding.
    let quad = |fa: u32, ma: u32, mb: u32, fb: u32, anchor: u32| -> [[u32; 3]; 2] {
        let if_branch = dist2(fa, mb) < dist2(ma, fb);
        if if_branch {
            [rot_to(anchor, [fa, ma, mb]), rot_new([fa, mb, fb])]
        } else {
            [rot_new([fa, ma, fb]), rot_new([ma, mb, fb])]
        }
    };
    let mut ind: Vec<[u32; 3]> = Vec::new();
    for &([pa, pb, pc], s) in &tris {
        match s {
            [None, None, None] => ind.push([pa, pb, pc]),
            [Some(m), None, None] => ind.push([pa, m, pc]),
            [None, Some(m), None] => ind.push([pa, pb, m]),
            [None, None, Some(m)] => ind.push([pa, pb, m]),
            [Some(m1), Some(m2), None] => ind.push([pb, m2, m1]),
            [None, Some(m2), Some(m3)] => ind.push([pc, m3, m2]),
            [Some(m1), None, Some(m3)] => ind.push([pa, m1, m3]),
            [Some(m1), Some(m2), Some(m3)] => ind.push([m1, m2, m3]),
        }
    }
    for &([pa, pb, pc], s) in &tris {
        match s {
            [None, None, None] => {}
            [Some(m), None, None] => ind.push([m, pb, pc]),
            [None, Some(m), None] => ind.push([m, pc, pa]),
            [None, None, Some(m)] => ind.push([m, pb, pc]),
            [Some(m1), Some(m2), None] => ind.extend(quad(pa, m1, m2, pc, m1)),
            [None, Some(m2), Some(m3)] => ind.extend(quad(pb, m2, m3, pa, m2)),
            [Some(m1), None, Some(m3)] => ind.extend(quad(pc, m3, m1, pb, m1)),
            [Some(m1), Some(m2), Some(m3)] => {
                ind.push([pa, m1, m3]);
                ind.push([m1, pb, m2]);
                ind.push([m2, pc, m3]);
            }
        }
    }
    out.indices = ind;
    out
}

/// Each new vert is derived from its split edge's endpoints, and appended to every bone it
/// references. `None` if a stream is missing or the grown counts overflow the format's u16 fields;
/// call [`ff13_formats::wrb::serialize_recompute`] afterwards to fix chunk sizes.
pub fn splice_mesh(mesh: &mut Chunk, sub: &Subdivision) -> Option<()> {
    let (mut pos_i, mut idx_i) = (None, None);
    let (mut stride, mut pos_off) = (0usize, 0usize);
    for (i, c) in mesh.children().iter().enumerate() {
        if let Some(s) = c.as_stms() {
            if let Some(off) = s.position_offset() {
                pos_i = Some(i);
                stride = s.stride as usize;
                pos_off = off;
            } else if s.is_index_buffer() {
                idx_i = Some(i);
            }
        }
    }
    let (pos_i, idx_i) = (pos_i?, idx_i?);
    let nnew = sub.new_positions.len();
    let orig_vert_count = BE::read_u32(&mesh.children()[pos_i].leaf()?[4..8]) as usize;
    // index values and the HEAD counts are u16; past 65535 verts/prims the mesh cannot serialize
    if orig_vert_count + nnew > u16::MAX as usize || sub.indices.len() > u16::MAX as usize {
        return None;
    }
    // each new vert's (bone-palette index, weight) influences, collected here,
    // merged into ENVD lists below
    let mut new_skin: Vec<Vec<(u8, u8)>> = vec![Vec::new(); nnew];

    {
        let leaf = mesh.children()[pos_i].leaf()?.to_vec();
        let elem_count = BE::read_u32(&leaf[0..4]) as usize;
        let vert_count = BE::read_u32(&leaf[4..8]) as usize;
        let decl_end = 16 + elem_count * 16;
        // element decl: usage -> byte offset within a vertex
        let mut uv_offs: Vec<usize> = Vec::new();
        let (mut norm_off, mut tan_off, mut bidx_off, mut bwt_off, mut col_off) =
            (None, None, None, None, None);
        for i in 0..elem_count {
            let e = 16 + i * 16;
            let off = BE::read_u32(&leaf[e..e + 4]) as usize;
            let usage = (BE::read_u32(&leaf[e + 12..e + 16]) >> 16) as u8;
            match usage {
                8 | 9 => uv_offs.push(off), // texcoords (BE float16 ×2), midpoint + flush quirk
                2 => norm_off = Some(off),  // normal (+127-biased int8 ×3)
                13 => tan_off = Some(off),  // tangent (+127-biased int8 ×3 + handedness)
                15 => bidx_off = Some(off), // bone palette indices (u8 ×4)
                14 => bwt_off = Some(off),  // bone weights (u8 ×4, sum 255)
                3 => col_off = Some(off),   // vertex color (RGBA u8), per-byte mean
                _ => {}
            }
        }
        let vbuf = &leaf[decl_end..decl_end + vert_count * stride];
        let mut out = Vec::with_capacity(leaf.len() + nnew * stride);
        out.extend_from_slice(&leaf[..decl_end]);
        BE::write_u32(&mut out[4..8], (vert_count + nnew) as u32);
        let existing_start = out.len();
        out.extend_from_slice(vbuf);
        // rewrite affected corners to bundle morph (#Q requantization)
        if !sub.corner_overrides.is_empty() {
            for v in 0..vert_count {
                let p = existing_start + v * stride + pos_off;
                let cur = [
                    i16::from_be_bytes([out[p], out[p + 1]]),
                    i16::from_be_bytes([out[p + 2], out[p + 3]]),
                    i16::from_be_bytes([out[p + 4], out[p + 5]]),
                ];
                if let Some(new) = sub.corner_overrides.get(&cur) {
                    for c in 0..3 {
                        let b = new[c].to_be_bytes();
                        out[p + c * 2] = b[0];
                        out[p + c * 2 + 1] = b[1];
                    }
                }
            }
        }
        for (k, raw) in sub.new_positions.iter().enumerate() {
            let (ti, tj) = sub.new_vert_edge[k];
            let (ea, eb) = (ti as usize * stride, tj as usize * stride);
            let base = out.len();
            out.extend_from_slice(&vbuf[ea..ea + stride]); // template from endpoint ti
            for c in 0..3 {
                let b = raw[c].to_be_bytes();
                out[base + pos_off + c * 2] = b[0];
                out[base + pos_off + c * 2 + 1] = b[1];
            }
            // Halves below the smallest normal, zero included, flush to 0x0800 rather than 0.
            for &uo in &uv_offs {
                for h in 0..2 {
                    let p = uo + h * 2;
                    let va = ff13_formats::wrb::half_be(vbuf[ea + p], vbuf[ea + p + 1]);
                    let vb = ff13_formats::wrb::half_be(vbuf[eb + p], vbuf[eb + p + 1]);
                    let m = ff13_formats::wrb::f16_be((va + vb) / 2.0);
                    let bits = ((m[0] as u16) << 8) | m[1] as u16;
                    let enc = if bits & 0x7C00 == 0 { [0x08, 0x00] } else { m };
                    out[base + p] = enc[0];
                    out[base + p + 1] = enc[1];
                }
            }
            // The +127-biased int8 codec is asymmetric: decode `(byte-127)/128` when non-negative
            // and `/127` otherwise, and re-encode at that same per-sign scale.
            let dec = |b: u8| -> f64 {
                let n = b as i32 - 127;
                if n >= 0 {
                    n as f64 / 128.0
                } else {
                    n as f64 / 127.0
                }
            };
            let enc = |v: f64| -> u8 {
                let v = v.clamp(-1.0, 1.0);
                let s = if v >= 0.0 { 128.0 } else { 127.0 };
                ((v * s).round() as i32 + 127).clamp(0, 255) as u8
            };
            let mut blend = |off: usize| {
                let na = [
                    dec(vbuf[ea + off]),
                    dec(vbuf[ea + off + 1]),
                    dec(vbuf[ea + off + 2]),
                ];
                let nb = [
                    dec(vbuf[eb + off]),
                    dec(vbuf[eb + off + 1]),
                    dec(vbuf[eb + off + 2]),
                ];
                let la = (na[0] * na[0] + na[1] * na[1] + na[2] * na[2]).sqrt();
                let lb = (nb[0] * nb[0] + nb[1] * nb[1] + nb[2] * nb[2]).sqrt();
                if la < 1e-3 || lb < 1e-3 {
                    for c in 0..3 {
                        out[base + off + c] = 127;
                    }
                    return;
                }
                let sign = if vbuf[ea + off + 3] == vbuf[eb + off + 3] {
                    1.0
                } else {
                    -1.0
                };
                let mut r = [0f64; 3];
                for c in 0..3 {
                    r[c] = na[c] * 0.5 * sign / la + nb[c] * 0.5 / lb;
                }
                let rl = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
                if rl < 1e-5 {
                    // endpoints cancel (equal tangent, opposite handedness) -> zero (byte 127)
                    for c in 0..3 {
                        out[base + off + c] = 127;
                    }
                } else {
                    for c in 0..3 {
                        out[base + off + c] = enc(r[c] / rl);
                    }
                }
            };
            if let Some(no) = norm_off {
                blend(no);
            }
            if let Some(to) = tan_off {
                blend(to);
                out[base + to + 3] = vbuf[eb + to + 3];
            }
            // 0xFF on either side passes through instead of averaging.
            if let Some(co) = col_off {
                for c in 0..4 {
                    let (a, b) = (vbuf[ea + co + c], vbuf[eb + co + c]);
                    out[base + co + c] = if a == 0xff || b == 0xff {
                        0xff
                    } else {
                        ((a as u16 + b as u16) >> 1) as u8
                    };
                }
            }

            if let (Some(io), Some(wo)) = (bidx_off, bwt_off) {
                // Tags record which endpoint a bone came from, which the tie fixup below needs.
                let mut acc: Vec<(u8, u32, u8)> = Vec::with_capacity(8);
                for s in 0..4 {
                    let idx = vbuf[ea + io + s];
                    if idx == 0xff {
                        continue;
                    }
                    let w = vbuf[ea + wo + s] as u32;
                    if w == 0 {
                        continue;
                    }
                    acc.push((idx, w, 1));
                }
                for s in 0..4 {
                    let idx = vbuf[eb + io + s];
                    let w = vbuf[eb + wo + s] as u32;
                    if let Some(e) = acc.iter_mut().find(|e| e.0 == idx) {
                        e.1 += w;
                        e.2 = 3;
                        continue;
                    }
                    if idx == 0xff || w == 0 {
                        continue;
                    }
                    acc.push((idx, w, 2));
                }
                // A strict `>` keeps equal weights in combined-list order.
                for i in 0..acc.len() {
                    let mut mx = i;
                    for j in (i + 1)..acc.len() {
                        if acc[j].1 > acc[mx].1 {
                            mx = j;
                        }
                    }
                    acc.swap(i, mx);
                }
                // When the 4th kept and 5th dropped weights tie, slot 3 goes to the geometrically
                // larger endpoint, measured as `x²+z²`; excluding y is deliberate, not an oversight.
                if acc.len() > 4 && acc[3].1 == acc[4].1 {
                    let (a3, a4) = (acc[3], acc[4]);
                    let pos_sq = |eo: usize| -> f32 {
                        let x = i16::from_be_bytes([vbuf[eo + pos_off], vbuf[eo + pos_off + 1]])
                            as f32
                            / 32767.0;
                        let z = i16::from_be_bytes([vbuf[eo + pos_off + 4], vbuf[eo + pos_off + 5]])
                            as f32
                            / 32767.0;
                        x * x + z * z
                    };
                    let (d, bv) = (pos_sq(ea), pos_sq(eb));
                    let take4 = if d > bv {
                        if a3.2 == 2 {
                            true
                        } else if a4.2 != a3.2 {
                            false
                        } else {
                            a3.0 > a4.0
                        }
                    } else if bv > d {
                        if a3.2 == 1 {
                            true
                        } else if a4.2 != a3.2 {
                            false
                        } else {
                            a3.0 > a4.0
                        }
                    } else {
                        a3.0 > a4.0
                    };
                    if take4 {
                        acc.swap(3, 4);
                    }
                }
                acc.truncate(4);
                let n = acc.len();
                let tot: u32 = acc.iter().map(|e| e.1).sum::<u32>().max(1);
                // Truncates rather than rounds, and slot 0 absorbs the leftover.
                let scale = 255.2 / tot as f64;
                let mut wts: Vec<i32> = acc
                    .iter()
                    .map(|e| (e.1 as f64 * scale + 0.45).trunc() as i32)
                    .collect();
                let accsum: i32 = wts.iter().sum();
                let def = 255 - accsum;
                if def == 0 {
                } else if def <= 1 {
                    if n > 0 {
                        wts[0] += def;
                    }
                } else if n > 1 {
                    wts[1] += 1;
                    wts[0] += 254 - accsum;
                } else if n > 0 {
                    wts[0] += def;
                }
                for s in 0..4 {
                    if let Some(&(idx, _, _)) = acc.get(s) {
                        let w = wts[s].clamp(0, 255) as u8;
                        out[base + io + s] = idx;
                        out[base + wo + s] = w;
                        new_skin[k].push((idx, w));
                    } else {
                        out[base + io + s] = 0xff;
                        out[base + wo + s] = 0;
                    }
                }
            }
        }
        mesh.children_mut()[pos_i].set_leaf(out);
    }

    {
        let leaf = mesh.children()[idx_i].leaf()?.to_vec();
        let elem_count = BE::read_u32(&leaf[0..4]) as usize;
        let decl_end = 16 + elem_count * 16;
        let n = sub.indices.len() * 3;
        let mut out = Vec::with_capacity(decl_end + n * 2);
        out.extend_from_slice(&leaf[..decl_end]);
        BE::write_u32(&mut out[4..8], n as u32);
        for t in &sub.indices {
            for &v in t {
                out.extend_from_slice(&(v as u16).to_be_bytes());
            }
        }
        mesh.children_mut()[idx_i].set_leaf(out);
    }

    envd_append(mesh, &new_skin, orig_vert_count);

    // HEAD: per-mesh draw descriptor caches vertex/primitive counts as BE u16
    // at content +8 and +10; update them.
    for ci in 0..mesh.children().len() {
        if mesh.children()[ci].magic() != *b"HEAD" {
            continue;
        }
        let mut h = mesh.children()[ci].content();
        if h.len() >= 12 {
            BE::write_u16(&mut h[8..10], (orig_vert_count + nnew) as u16);
            BE::write_u16(&mut h[10..12], sub.indices.len() as u16);
            mesh.children_mut()[ci].set_content(h);
        }
    }
    Some(())
}

/// Apply one `:~` to a whole `.trb`: subdivide submesh `(m,s)` via the named
/// bundle entry, splice, re-serialize. `(m,s)` = MDL[m] (LOD) then mesh `s` (flat
/// MESH walk = meshes in MDL[0..m]+s). `None` if model/bundle/op don't line up.
#[cfg(test)]
pub(crate) fn apply_op(
    trb_bytes: &[u8],
    bundle: &[u8],
    entry_substr: &str,
    m: usize,
    s: usize,
) -> Option<Vec<u8>> {
    use ff13_formats::trb::Trb;
    use ff13_formats::wrb;
    let t = Trb::parse(trb_bytes).ok()?;
    let wrb_idx = t.find_resource(b"SEDBwrb")?;
    let res = t.resource_data(wrb_idx)?.to_vec();
    let mut root = wrb::parse(&res).ok()?;

    let target = flat_mesh_index(&root, m, s)?;
    let (pos, raw, idx) = read_submesh(&root, target)?;
    let ent = crate::modbundle::entries(bundle)
        .into_iter()
        .find(|e| e.name.contains(entry_substr))?;
    let sub = subdivide(&pos, &raw, &idx, &ent.vertices(), 0.005, None, &[]);

    let mut i = 0;
    let mesh = nth_mesh_mut(&mut root, target, &mut i)?;
    splice_mesh(mesh, &sub)?;
    let new_res = wrb::serialize_recompute(&res, &root);
    t.serialize_replacing(wrb_idx, &new_res).ok()
}

/// The position `STMS` (the `usage==0` stream) child of a `MESH` chunk.
fn mesh_pos_stms(mesh: &mut Chunk) -> Option<&mut Chunk> {
    mesh.children_mut()
        .iter_mut()
        .find(|c| c.as_stms().is_some_and(|s| s.position_offset().is_some()))
}

/// `:L` region-gated radial deform: scales each vertex's perpendicular offset from the `p1`-`p2`
/// axis, leaving the axial component alone. `arg2`'s falloff-profile nibbles are unmodelled, since
/// shipped ops all pass 0.
fn l_deform(
    mesh: &mut Chunk,
    p1: [f32; 3],
    p2: [f32; 3],
    scale: f32,
    falloff: f32,
    region: Option<&[Region]>,
) {
    let Some(stms) = mesh_pos_stms(mesh) else {
        return;
    };
    let Some((pos_off, stride)) = stms
        .as_stms()
        .and_then(|s| s.position_offset().map(|o| (o, s.stride as usize)))
    else {
        return;
    };
    let Some(leaf) = stms.leaf() else { return };
    let elem_count = BE::read_u32(&leaf[0..4]) as usize;
    let vert_count = BE::read_u32(&leaf[4..8]) as usize;
    let decl_end = 16 + elem_count * 16;
    let mut out = leaf.to_vec();
    let d = [p1[0] - p2[0], p1[1] - p2[1], p1[2] - p2[2]];
    let lsq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if lsq <= 0.0 {
        return;
    }
    let inv = 1.0 / lsq.sqrt();
    let a = [d[0] * inv, d[1] * inv, d[2] * inv];
    for v in 0..vert_count {
        let base = decl_end + v * stride + pos_off;
        // World (x,y,z) maps to position slots (0,2,1), and everything below is in that frame.
        let w = [
            BE::read_i16(&out[base..]) as f32 / 32767.0,
            BE::read_i16(&out[base + 4..]) as f32 / 32767.0,
            BE::read_i16(&out[base + 2..]) as f32 / 32767.0,
        ];
        if !region.is_none_or(|rg| region_contains(w, rg)) {
            continue;
        }
        let t = (w[0] - p1[0]) * a[0] + (w[1] - p1[1]) * a[1] + (w[2] - p1[2]) * a[2];
        let f = [p1[0] + t * a[0], p1[1] + t * a[1], p1[2] + t * a[2]];
        let perp = [w[0] - f[0], w[1] - f[1], w[2] - f[2]];
        let r = (perp[0] * perp[0] + perp[1] * perp[1] + perp[2] * perp[2]).sqrt();
        if r > falloff {
            continue;
        }
        let nw = [
            f[0] + perp[0] * scale,
            f[1] + perp[1] * scale,
            f[2] + perp[2] * scale,
        ];
        // write world (x,y,z) back to slots (0,2,1)
        let enc = encode_pos([nw[0], nw[2], nw[1]]);
        for c in 0..3 {
            let b = enc[c].to_be_bytes();
            out[base + c * 2] = b[0];
            out[base + c * 2 + 1] = b[1];
        }
    }
    stms.set_leaf(out);
}

/// Append `new_skin[k]`'s `(bone-palette-index, weight)` pairs to the matching
/// bones' ENVD lists. New vert `k` has id `nold+k`; the Nth ENVD child is bone
/// index N (usage-15). ENVD layout: 16-byte header with TWO identical `(count,
/// index_off, weight_off)` BE-u16 triples (@2/4/6 and @8/10/12), bone name
/// `[16, index_off)`, INDEX array (BE u16, padded even with 0x0000 terminator),
/// WEIGHT array (u8, exactly `count`). ENVD is rebuilt from the per-vertex bone streams on save,
/// so every vert-appending op has to mirror it.
fn envd_append(mesh: &mut Chunk, new_skin: &[Vec<(u8, u8)>], nold: usize) {
    let nnew = new_skin.len();
    let mut bone = 0usize;
    for ci in 0..mesh.children().len() {
        if mesh.children()[ci].magic() != *b"ENVD" {
            continue;
        }
        let bi = bone;
        bone += 1;
        let adds: Vec<(u16, u8)> = (0..nnew)
            .flat_map(|k| {
                let v = (nold + k) as u16;
                new_skin[k]
                    .iter()
                    .filter(move |&&(p, _)| p as usize == bi)
                    .map(move |&(_, w)| (v, w))
            })
            .collect();
        if adds.is_empty() {
            continue;
        }
        let leaf = mesh.children()[ci].content(); // ENVD may parse as container
        let count = BE::read_u16(&leaf[2..4]) as usize;
        let index_off = BE::read_u16(&leaf[4..6]) as usize;
        let weight_off = BE::read_u16(&leaf[6..8]) as usize;
        if index_off < 16 || weight_off + count > leaf.len() {
            continue;
        }
        // existing real influences (index array may carry one terminator beyond `count`)
        let mut infl: Vec<(u16, u8)> = (0..count)
            .map(|i| {
                (
                    BE::read_u16(&leaf[index_off + i * 2..]),
                    leaf[weight_off + i],
                )
            })
            .collect();
        if infl.last().map(|e| e.0) == Some(0xFFFF) {
            infl.pop();
        }
        infl.extend(adds);
        let real = infl.len();
        let even_idx = real + (real & 1); // index array padded to even length
        let new_weight_off = index_off + even_idx * 2;
        let mut nl = leaf[..index_off].to_vec(); // 16-byte header + bone name
        BE::write_u16(&mut nl[2..4], real as u16);
        BE::write_u16(&mut nl[6..8], new_weight_off as u16);
        if index_off >= 14 {
            BE::write_u16(&mut nl[8..10], real as u16); // duplicate triple
            BE::write_u16(&mut nl[12..14], new_weight_off as u16);
        }
        for &(v, _) in &infl {
            nl.extend_from_slice(&v.to_be_bytes());
        }
        for _ in real..even_idx {
            nl.extend_from_slice(&0u16.to_be_bytes()); // index-array terminator
        }
        nl.extend(infl.iter().map(|&(_, w)| w)); // weights: exactly `real`
        while !nl.len().is_multiple_of(4) {
            nl.push(0); // pad to 4-byte boundary (counted in size@8)
        }
        mesh.children_mut()[ci].set_content(nl);
    }
}

/// Child indices of a MESH's (vertex `STMS`, index `STMS`).
fn stream_children(mesh: &Chunk) -> Option<(usize, usize)> {
    let (mut pos, mut idx) = (None, None);
    for (i, c) in mesh.children().iter().enumerate() {
        if let Some(s) = c.as_stms() {
            if s.position_offset().is_some() {
                pos = Some(i);
            } else if s.is_index_buffer() {
                idx = Some(i);
            }
        }
    }
    Some((pos?, idx?))
}

/// The current vertex count (the position `STMS` leaf's count field at +4).
fn vert_count(mesh: &Chunk, pos_i: usize) -> usize {
    BE::read_u32(&mesh.children()[pos_i].leaf().unwrap()[4..8]) as usize
}

/// Decode the index `STMS` into triangle indices.
fn read_indices(mesh: &Chunk, idx_i: usize) -> Option<Vec<u16>> {
    let leaf = mesh.children()[idx_i].leaf()?;
    let elem_count = BE::read_u32(&leaf[0..4]) as usize;
    let decl_end = 16 + elem_count * 16;
    let n = BE::read_u32(&leaf[4..8]) as usize;
    Some(
        (0..n)
            .map(|i| BE::read_u16(&leaf[decl_end + i * 2..]))
            .collect(),
    )
}

/// Write a new index list back into the index `STMS` leaf (count + BE u16 data).
fn write_indices(mesh: &mut Chunk, idx_i: usize, inds: &[u16]) -> Option<()> {
    let leaf = mesh.children()[idx_i].leaf()?.to_vec();
    let elem_count = BE::read_u32(&leaf[0..4]) as usize;
    let decl_end = 16 + elem_count * 16;
    let mut out = leaf[..decl_end].to_vec();
    BE::write_u32(&mut out[4..8], inds.len() as u32);
    for &v in inds {
        out.extend_from_slice(&v.to_be_bytes());
    }
    mesh.children_mut()[idx_i].set_leaf(out);
    Some(())
}

/// Update HEAD's cached counts (vertex @ content+8, primitive @ +10, BE u16).
fn update_head(mesh: &mut Chunk, verts: Option<usize>, prims: Option<usize>) {
    for ci in 0..mesh.children().len() {
        if mesh.children()[ci].magic() != *b"HEAD" {
            continue;
        }
        let mut h = mesh.children()[ci].content();
        if h.len() >= 12 {
            if let Some(v) = verts {
                BE::write_u16(&mut h[8..10], v as u16);
            }
            if let Some(p) = prims {
                BE::write_u16(&mut h[10..12], p as u16);
            }
            mesh.children_mut()[ci].set_content(h);
        }
    }
}

/// Resolve a (possibly negative) vertex index vs the live count: `i>=0` -> `i`;
/// `i<0` -> `count+i` (`-1` = last vertex).
fn resolve_index(i: i32, count: usize) -> i32 {
    if i < 0 { count as i32 + i } else { i }
}

/// `:P` Edit Poly: overwrite the tri matching `a`, in buffer winding `a0,a2,a1`, with `b`. An
/// all-zero `a` replaces the last tri instead; no match is a no-op.
fn edit_poly(mesh: &mut Chunk, a: [i32; 3], b: [i32; 3]) -> Option<()> {
    let (pos_i, idx_i) = stream_children(mesh)?;
    let vc = vert_count(mesh, pos_i);
    let ar = a.map(|x| resolve_index(x, vc) as u16);
    let br = b.map(|x| resolve_index(x, vc) as u16);
    let mut inds = read_indices(mesh, idx_i)?;
    let tris = inds.len() / 3;
    if (a[0] as u16 | a[1] as u16 | a[2] as u16) == 0 {
        if tris == 0 {
            return Some(());
        }
        let o = (tris - 1) * 3;
        inds[o..o + 3].copy_from_slice(&br);
        return write_indices(mesh, idx_i, &inds);
    }
    for t in 0..tris {
        let o = t * 3;
        if inds[o..o + 3] == ar {
            inds[o..o + 3].copy_from_slice(&br);
            return write_indices(mesh, idx_i, &inds);
        }
    }
    Some(())
}

/// `:N` Add Triangle: append a tri and bump the HEAD primitive count.
fn add_triangle(mesh: &mut Chunk, v: [i32; 3]) -> Option<()> {
    let (pos_i, idx_i) = stream_children(mesh)?;
    let vc = vert_count(mesh, pos_i);
    let mut inds = read_indices(mesh, idx_i)?;
    for &x in &v {
        inds.push(resolve_index(x, vc) as u16);
    }
    write_indices(mesh, idx_i, &inds)?;
    update_head(mesh, None, Some(inds.len() / 3));
    Some(())
}

/// `:T` Add Vertex: append a vertex interpolated between `va` and `vb`, positions and normals by
/// `f1` and UVs by `f2`. Bumps the HEAD vertex count but does NOT touch the index buffer.
fn add_vertex(mesh: &mut Chunk, va: i32, vb: i32, f1: f32, f2: f32) -> Option<usize> {
    let (pos_i, _) = stream_children(mesh)?;
    let leaf = mesh.children()[pos_i].leaf()?.to_vec();
    let elem_count = BE::read_u32(&leaf[0..4]) as usize;
    let vc = BE::read_u32(&leaf[4..8]) as usize;
    if vc + 1 > u16::MAX as usize {
        return None; // HEAD vertex count and index refs are u16
    }
    let stride = mesh.children()[pos_i].as_stms()?.stride as usize;
    let decl_end = 16 + elem_count * 16;
    let (mut pos_off, mut uv_offs) = (None, Vec::new());
    let (mut norm_off, mut tan_off, mut col_off, mut bidx_off, mut bwt_off) =
        (None, None, None, None, None);
    for i in 0..elem_count {
        let e = 16 + i * 16;
        let off = BE::read_u32(&leaf[e..e + 4]) as usize;
        let usage = (BE::read_u32(&leaf[e + 12..e + 16]) >> 16) as u8;
        match usage {
            0 => pos_off = Some(off),
            8 | 9 => uv_offs.push(off),
            2 => norm_off = Some(off),
            13 => tan_off = Some(off),
            3 => col_off = Some(off),
            15 => bidx_off = Some(off),
            14 => bwt_off = Some(off),
            _ => {}
        }
    }
    let pos_off = pos_off?;
    let vbuf = &leaf[decl_end..decl_end + vc * stride];
    let ea = resolve_index(va, vc) as usize * stride;
    let eb = resolve_index(vb, vc) as usize * stride;
    let mut nv = vbuf[ea..ea + stride].to_vec(); // template from A

    // position = lerp of SNORM-decoded endpoints, re-encoded (truncate)
    let dpos = |o: usize, c: usize| {
        i16::from_be_bytes([vbuf[o + pos_off + c * 2], vbuf[o + pos_off + c * 2 + 1]]) as f32
            / 32767.0
    };
    let p = [0, 1, 2].map(|c| (1.0 - f1) * dpos(ea, c) + f1 * dpos(eb, c));
    let pe = encode_pos(p);
    for c in 0..3 {
        nv[pos_off + c * 2..pos_off + c * 2 + 2].copy_from_slice(&pe[c].to_be_bytes());
    }
    // UV = lerp by f2 (float16) with the denormal->0x0800 flush quirk
    for &uo in &uv_offs {
        for h in 0..2 {
            let pp = uo + h * 2;
            let av = ff13_formats::wrb::half_be(vbuf[ea + pp], vbuf[ea + pp + 1]);
            let bv = ff13_formats::wrb::half_be(vbuf[eb + pp], vbuf[eb + pp + 1]);
            let m = ff13_formats::wrb::f16_be((1.0 - f2) * av + f2 * bv);
            let bits = ((m[0] as u16) << 8) | m[1] as u16;
            let enc = if bits & 0x7C00 == 0 { [0x08, 0x00] } else { m };
            nv[pp..pp + 2].copy_from_slice(&enc);
        }
    }
    // normal/tangent = normalize(lerp(normalize(A)·sign, normalize(B))) in the
    // +127-biased int8 codec (same blend as `:~`; sign from handedness byte)
    let dec = |b: u8| -> f64 {
        let n = b as i32 - 127;
        if n >= 0 {
            n as f64 / 128.0
        } else {
            n as f64 / 127.0
        }
    };
    let enc = |v: f64| -> u8 {
        let v = v.clamp(-1.0, 1.0);
        let s = if v >= 0.0 { 128.0 } else { 127.0 };
        ((v * s).round() as i32 + 127).clamp(0, 255) as u8
    };
    // scoped so `blend`'s mut borrow of `nv` ends before the color pass
    {
        let mut blend = |off: usize| {
            let na = [
                dec(vbuf[ea + off]),
                dec(vbuf[ea + off + 1]),
                dec(vbuf[ea + off + 2]),
            ];
            let nb = [
                dec(vbuf[eb + off]),
                dec(vbuf[eb + off + 1]),
                dec(vbuf[eb + off + 2]),
            ];
            let la = (na[0] * na[0] + na[1] * na[1] + na[2] * na[2])
                .sqrt()
                .max(1e-12);
            let lb = (nb[0] * nb[0] + nb[1] * nb[1] + nb[2] * nb[2])
                .sqrt()
                .max(1e-12);
            let sign = if vbuf[ea + off + 3] == vbuf[eb + off + 3] {
                1.0
            } else {
                -1.0
            };
            let mut r = [0f64; 3];
            for c in 0..3 {
                r[c] = na[c] * (1.0 - f1 as f64) * sign / la + nb[c] * f1 as f64 / lb;
            }
            let rl = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt().max(1e-12);
            for c in 0..3 {
                nv[off + c] = enc(r[c] / rl);
            }
        };
        if let Some(no) = norm_off {
            blend(no);
        }
        if let Some(to) = tan_off {
            blend(to);
        }
    }
    // color = per-byte arithmetic mean, with 0xFF on either side passed through.
    if let Some(co) = col_off {
        for c in 0..4 {
            let (av, bv) = (vbuf[ea + co + c], vbuf[eb + co + c]);
            nv[co + c] = if av == 0xff || bv == 0xff {
                0xff
            } else {
                ((av as u16 + bv as u16) >> 1) as u8
            };
        }
    }
    // skin = top-4 merge of both endpoints' influences (same as `:~`).
    let mut skin: Vec<(u8, u8)> = Vec::new();
    if let (Some(io), Some(wo)) = (bidx_off, bwt_off) {
        let mut acc: Vec<(u8, u32)> = Vec::with_capacity(8);
        for &eo in &[ea, eb] {
            for s in 0..4 {
                let idx = vbuf[eo + io + s];
                if idx == 0xff {
                    continue;
                }
                let w = vbuf[eo + wo + s] as u32;
                if let Some(en) = acc.iter_mut().find(|e| e.0 == idx) {
                    en.1 += w;
                } else {
                    acc.push((idx, w));
                }
            }
        }
        let mut keep = acc.clone();
        keep.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
        keep.truncate(4);
        acc.retain(|e| keep.iter().any(|k| k.0 == e.0));
        for i in 0..acc.len() {
            let mut mx = i;
            for j in (i + 1)..acc.len() {
                if acc[j].1 > acc[mx].1 {
                    mx = j;
                }
            }
            acc.swap(i, mx);
        }
        let n = acc.len();
        let tot: u32 = acc.iter().map(|e| e.1).sum::<u32>().max(1);
        let scale = 255.2 / tot as f64;
        let mut wts: Vec<i32> = acc
            .iter()
            .map(|e| (e.1 as f64 * scale + 0.45).trunc() as i32)
            .collect();
        let accsum: i32 = wts.iter().sum();
        let def = 255 - accsum;
        if def == 0 {
        } else if def <= 1 {
            if n > 0 {
                wts[0] += def;
            }
        } else if n > 1 {
            wts[1] += 1;
            wts[0] += 254 - accsum;
        } else if n > 0 {
            wts[0] += def;
        }
        for s in 0..4 {
            if let Some(&(idx, _)) = acc.get(s) {
                let w = wts[s].clamp(0, 255) as u8;
                nv[io + s] = idx;
                nv[wo + s] = w;
                skin.push((idx, w));
            } else {
                nv[io + s] = 0xff;
                nv[wo + s] = 0;
            }
        }
    }

    let mut out = leaf[..decl_end + vc * stride].to_vec();
    BE::write_u32(&mut out[4..8], (vc + 1) as u32);
    out.extend_from_slice(&nv);
    mesh.children_mut()[pos_i].set_leaf(out);
    envd_append(mesh, &[skin], vc);
    update_head(mesh, Some(vc + 1), None);
    Some(vc)
}

/// `:R` Remove Vertex: drop the inclusive range `[start, end]` along with any tri referencing it,
/// then compact the stream and decrement the surviving indices above the range.
fn remove_verts(mesh: &mut Chunk, start: i32, end: i32) -> Option<(usize, usize)> {
    let (pos_i, idx_i) = stream_children(mesh)?;
    let leaf = mesh.children()[pos_i].leaf()?.to_vec();
    let elem_count = BE::read_u32(&leaf[0..4]) as usize;
    let vc = BE::read_u32(&leaf[4..8]) as usize;
    let stride = mesh.children()[pos_i].as_stms()?.stride as usize;
    let decl_end = 16 + elem_count * 16;
    let (s, e) = (resolve_index(start, vc), resolve_index(end, vc));
    if s < 0 || e < s || e as usize >= vc {
        return None;
    }
    let (s, e) = (s as usize, e as usize);
    let r = e - s + 1;

    let inds = read_indices(mesh, idx_i)?;
    let mut keep = Vec::with_capacity(inds.len());
    for t in inds.chunks_exact(3) {
        if t.iter().any(|&i| (i as usize) >= s && (i as usize) <= e) {
            continue;
        }
        for &i in t {
            keep.push(if (i as usize) > e { i - r as u16 } else { i });
        }
    }
    write_indices(mesh, idx_i, &keep)?;

    let vbuf = &leaf[decl_end..decl_end + vc * stride];
    let mut out = leaf[..decl_end].to_vec();
    BE::write_u32(&mut out[4..8], (vc - r) as u32);
    out.extend_from_slice(&vbuf[..s * stride]);
    out.extend_from_slice(&vbuf[(e + 1) * stride..]);
    mesh.children_mut()[pos_i].set_leaf(out);

    for ci in 0..mesh.children().len() {
        if mesh.children()[ci].magic() != *b"ENVD" {
            continue;
        }
        let el = mesh.children()[ci].content();
        let count = BE::read_u16(&el[2..4]) as usize;
        let index_off = BE::read_u16(&el[4..6]) as usize;
        let weight_off = BE::read_u16(&el[6..8]) as usize;
        if index_off < 16 || weight_off + count > el.len() {
            continue;
        }
        let mut infl: Vec<(u16, u8)> = (0..count)
            .map(|i| (BE::read_u16(&el[index_off + i * 2..]), el[weight_off + i]))
            .collect();
        infl.retain(|&(v, _)| !((v as usize) >= s && (v as usize) <= e));
        for x in infl.iter_mut() {
            if (x.0 as usize) > e {
                x.0 -= r as u16;
            }
        }
        let real = infl.len();
        let even_idx = real + (real & 1);
        let new_weight_off = index_off + even_idx * 2;
        let mut nl = el[..index_off].to_vec();
        BE::write_u16(&mut nl[2..4], real as u16);
        BE::write_u16(&mut nl[6..8], new_weight_off as u16);
        if index_off >= 14 {
            BE::write_u16(&mut nl[8..10], real as u16);
            BE::write_u16(&mut nl[12..14], new_weight_off as u16);
        }
        for &(v, _) in &infl {
            nl.extend_from_slice(&v.to_be_bytes());
        }
        for _ in real..even_idx {
            nl.extend_from_slice(&0u16.to_be_bytes());
        }
        nl.extend(infl.iter().map(|&(_, w)| w));
        while !nl.len().is_multiple_of(4) {
            nl.push(0);
        }
        mesh.children_mut()[ci].set_content(nl);
    }

    update_head(mesh, Some(vc - r), Some(keep.len() / 3));
    Some((s, e))
}

/// `:J` Shift Vertex Data: overwrite each vertex near a control's `pos` with that control's
/// `morph`. Positions only, with no geometry change.
fn shift_vertex_data(mesh: &mut Chunk, controls: &[ControlVertex]) -> Option<()> {
    let (pos_i, _) = stream_children(mesh)?;
    let mut out = mesh.children()[pos_i].leaf()?.to_vec();
    let elem_count = BE::read_u32(&out[0..4]) as usize;
    let vc = BE::read_u32(&out[4..8]) as usize;
    let stride = mesh.children()[pos_i].as_stms()?.stride as usize;
    let decl_end = 16 + elem_count * 16;
    let mut pos_off = None;
    for i in 0..elem_count {
        let e = 16 + i * 16;
        if (BE::read_u32(&out[e + 12..e + 16]) >> 16) as u8 == 0 {
            pos_off = Some(BE::read_u32(&out[e..e + 4]) as usize);
        }
    }
    let pos_off = pos_off?;
    for v in 0..vc {
        let p = decl_end + v * stride + pos_off;
        let cur = [0, 1, 2]
            .map(|c| i16::from_be_bytes([out[p + c * 2], out[p + c * 2 + 1]]) as f32 / 32767.0);
        for cvx in controls {
            let d = (cur[0] - cvx.pos[0]).abs()
                + (cur[1] - cvx.pos[1]).abs()
                + (cur[2] - cvx.pos[2]).abs();
            if d < 1e-4 {
                let enc = encode_pos(cvx.morph);
                for c in 0..3 {
                    out[p + c * 2..p + c * 2 + 2].copy_from_slice(&enc[c].to_be_bytes());
                }
                break;
            }
        }
    }
    mesh.children_mut()[pos_i].set_leaf(out);
    Some(())
}

/// `None` if the op carries fewer than `n` args.
fn args_i32(op: &crate::modscript::Op, n: usize) -> Option<Vec<i32>> {
    if op.args.len() < n {
        return None;
    }
    op.args[..n].iter().map(|a| a.parse().ok()).collect()
}

/// Mutates geometry in memory and re-serializes once at the end; unimplemented op letters are
/// skipped. `sources` holds the combine source models' `.trb` bytes in `model_paths` order, which
/// only `:I` needs; pass `&[]` for scripts without a combine.
pub fn apply_script_with_sources(
    trb_bytes: &[u8],
    bundle: &[u8],
    script: &crate::modscript::Script,
    sources: &[Vec<u8>],
) -> Option<Vec<u8>> {
    use crate::modscript::VertexOp;
    use ff13_formats::trb::Trb;
    use ff13_formats::wrb;
    let t = Trb::parse(trb_bytes).ok()?;
    let wrb_idx = t.find_resource(b"SEDBwrb")?;
    let res = t.resource_data(wrb_idx)?.to_vec();
    let mut root = wrb::parse(&res).ok()?;

    let bundle_entries = crate::modbundle::entries(bundle);

    // Vertex provenance: per flat-mesh, `:T`-added verts `(src_a, src_b, vert_index)`
    // in CURRENT indexing (remapped on `:R`); a later `:~` reuses these by
    // source-edge key instead of creating duplicate split verts.
    let mut t_verts: HashMap<usize, Vec<(usize, usize, usize)>> = HashMap::new();

    // a geometry op on `(model, mesh)`: locate and apply `f` to its MESH; skip if unresolved
    macro_rules! on_mesh {
        ($m:expr, $s:expr, $mesh:ident => $body:block) => {{
            if let Some(flat) = flat_mesh_index(&root, $m, $s) {
                let mut i = 0;
                if let Some($mesh) = nth_mesh_mut(&mut root, flat, &mut i) {
                    $body
                }
            }
        }};
    }

    // `:e` populates regions + enables the gate; `:E,n` sets the enable word
    // (`:E,0` disables). A `:~` while enabled keeps only region-selected geometry.
    let mut regions: Vec<Region> = Vec::new();
    let mut region_on = false;

    for op in &script.ops {
        if op.tag == ":X" {
            break; // end-of-script terminator
        }
        if op.tag.starts_with(":/") {
            continue; // author directives (`:/G`, `:/SCULPTING`), no geometry
        }
        match op.letter() {
            Some('e') => {
                if let Some((r, reset)) = Region::parse(&op.args) {
                    if reset {
                        regions.clear();
                    }
                    regions.push(r);
                    region_on = true;
                }
            }
            Some('E') => {
                let n: i32 = op.args.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                region_on = (n & 0xFFFF) != 0;
            }
            Some('L') if op.args.len() >= 11 => {
                // :L,model,mesh,arg2,x1,y1,z1,x2,y2,z2,scale,falloff[,t0,t1]
                let (Ok(m), Ok(s)) = (op.args[0].parse::<usize>(), op.args[1].parse::<usize>())
                else {
                    continue;
                };
                let f = |i: usize| {
                    op.args
                        .get(i)
                        .and_then(|x| x.parse::<f32>().ok())
                        .unwrap_or(0.0)
                };
                let (p1, p2) = ([f(3), f(4), f(5)], [f(6), f(7), f(8)]);
                let (scale, falloff) = (f(9), f(10));
                let reg = region_on.then_some(regions.as_slice());
                on_mesh!(m, s, mesh => { l_deform(mesh, p1, p2, scale, falloff, reg); });
            }
            Some('V') => {
                let Some(v) = VertexOp::parse(op) else {
                    continue;
                };
                on_mesh!(v.model as usize, v.mesh as usize, mesh => {
                    if let Some(stms) = mesh_pos_stms(mesh) {
                        v.apply_to_stms(stms);
                    }
                });
            }
            Some('~') if op.args.len() >= 7 => {
                let (Ok(m), Ok(s)) = (op.args[0].parse::<usize>(), op.args[1].parse::<usize>())
                else {
                    continue;
                };
                let merge_eps = op.args[4].parse::<f32>().unwrap_or(0.005);
                let entry = op.args[5].trim_end_matches(".txt");
                let Some(flat) = flat_mesh_index(&root, m, s) else {
                    continue;
                };
                let Some((pos, raw, idx)) = read_submesh(&root, flat) else {
                    continue;
                };
                let Some(ent) = bundle_entries.iter().find(|e| e.name.contains(entry)) else {
                    continue;
                };
                let reg = region_on.then_some(regions.as_slice());
                let reuse = t_verts.get(&flat).map(Vec::as_slice).unwrap_or(&[]);
                let sub = subdivide(&pos, &raw, &idx, &ent.vertices(), merge_eps, reg, reuse);
                let mut i = 0;
                if let Some(mesh) = nth_mesh_mut(&mut root, flat, &mut i) {
                    splice_mesh(mesh, &sub)?;
                }
            }
            Some('P') => {
                let Some(a) = args_i32(op, 8) else { continue };
                on_mesh!(a[0] as usize, a[1] as usize, mesh => {
                    let _ = edit_poly(mesh, [a[2], a[3], a[4]], [a[5], a[6], a[7]]);
                });
            }
            Some('N') => {
                let Some(a) = args_i32(op, 5) else { continue };
                on_mesh!(a[0] as usize, a[1] as usize, mesh => {
                    let _ = add_triangle(mesh, [a[2], a[3], a[4]]);
                });
            }
            Some('T') => {
                let Some(a) = args_i32(op, 4) else { continue };
                let (Ok(f1), Ok(f2)) = (op.args[4].parse::<f32>(), op.args[5].parse::<f32>())
                else {
                    continue;
                };
                let Some(flat) = flat_mesh_index(&root, a[0] as usize, a[1] as usize) else {
                    continue;
                };
                let mut i = 0;
                if let Some(mesh) = nth_mesh_mut(&mut root, flat, &mut i)
                    && let Some(vidx) = add_vertex(mesh, a[2], a[3], f1, f2)
                {
                    // key the new vert to its source edge (resolved vs pre-add count = vidx)
                    let sa = resolve_index(a[2], vidx) as usize;
                    let sb = resolve_index(a[3], vidx) as usize;
                    t_verts.entry(flat).or_default().push((sa, sb, vidx));
                }
            }
            Some('R') => {
                let Some(a) = args_i32(op, 4) else { continue };
                let Some(flat) = flat_mesh_index(&root, a[0] as usize, a[1] as usize) else {
                    continue;
                };
                let mut i = 0;
                if let Some(mesh) = nth_mesh_mut(&mut root, flat, &mut i)
                    && let Some((s, e)) = remove_verts(mesh, a[2], a[3])
                {
                    // remap tracked `:T` verts through the removal (drop removed, shift above-range down)
                    let r = e - s + 1;
                    if let Some(list) = t_verts.get_mut(&flat) {
                        list.retain_mut(|(sa, sb, v)| {
                            if [*sa, *sb, *v].iter().any(|&x| x >= s && x <= e) {
                                return false;
                            }
                            for x in [sa, sb, v] {
                                if *x > e {
                                    *x -= r;
                                }
                            }
                            true
                        });
                    }
                }
            }
            Some('J') if op.args.len() >= 3 => {
                let (Ok(m), Ok(s)) = (op.args[0].parse::<usize>(), op.args[1].parse::<usize>())
                else {
                    continue;
                };
                let name = op.args[2].trim_end_matches(".txt");
                let Some(ent) = bundle_entries.iter().find(|e| e.name.contains(name)) else {
                    continue;
                };
                let cvs = ent.vertices();
                on_mesh!(m, s, mesh => {
                    let _ = shift_vertex_data(mesh, &cvs);
                });
            }
            Some('D') if op.args.len() >= 2 => {
                // Delete Mesh: remove the flat MESH chunk and decrement its MDL's mesh count.
                let (Ok(m), Ok(s)) = (op.args[0].parse::<usize>(), op.args[1].parse::<usize>())
                else {
                    continue;
                };
                if let Some(flat) = flat_mesh_index(&root, m, s) {
                    delete_mesh(&mut root, flat);
                }
            }
            Some('I') if op.args.len() >= 4 => {
                // Insert Mesh: copy a MESH from a source WRB into the output MDL's position.
                let a: Vec<usize> = op.args[..4]
                    .iter()
                    .filter_map(|x| x.trim().parse().ok())
                    .collect();
                if a.len() < 4 {
                    continue;
                }
                let (dst_m, dst_s, src_m, src_s) = (a[0], a[1], a[2], a[3]);
                let Some(src_trb) = sources.get(src_m) else {
                    continue;
                };
                let Some(src_mesh) = Trb::parse(src_trb)
                    .ok()
                    .and_then(|st| st.find_resource(b"SEDBwrb").map(|i| (st, i)))
                    .and_then(|(st, i)| st.resource_data(i).map(<[u8]>::to_vec))
                    .and_then(|wb| wrb::parse(&wb).ok())
                    .and_then(|sr| clone_nth_mesh(&sr, src_s))
                else {
                    continue;
                };
                insert_mesh(&mut root, dst_m, dst_s, src_mesh);
            }
            _ => {} // :u, :E, :e (region state), :O, not yet reimplemented
        }
    }
    let new_res = wrb::serialize_recompute(&res, &root);
    t.serialize_replacing(wrb_idx, &new_res).ok()
}

fn flat_mesh_index(root: &Chunk, m: usize, s: usize) -> Option<usize> {
    fn count_meshes(c: &Chunk, n: &mut usize) {
        if c.magic() == *b"MESH" && c.submesh().is_some() {
            *n += 1;
        }
        for k in c.children() {
            count_meshes(k, n);
        }
    }
    fn collect(c: &Chunk, per: &mut Vec<usize>) {
        // an MDL (LOD) is `MDL\0` or `MDL `, NOT the `MDLC` container
        if c.magic() == *b"MDL\0" || c.magic() == *b"MDL " {
            let mut n = 0;
            count_meshes(c, &mut n);
            per.push(n);
        } else {
            for k in c.children() {
                collect(k, per);
            }
        }
    }
    let mut per = Vec::new();
    collect(root, &mut per);
    if per.is_empty() {
        return Some(s);
    }
    Some(per.get(..m)?.iter().sum::<usize>() + s)
}

/// A submesh read back from a WRB: `(positions, raw-int16 positions, indices)`.
type Submesh = (Vec<[f32; 3]>, Vec<[i16; 3]>, Vec<u16>);

fn read_submesh(root: &Chunk, target: usize) -> Option<Submesh> {
    fn walk(c: &Chunk, i: &mut usize, tg: usize, out: &mut Option<Submesh>) {
        if c.magic() == *b"MESH"
            && let Some(sm) = c.submesh()
        {
            if *i == tg {
                let pos = (0..sm.positions.vert_count)
                    .filter_map(|v| sm.positions.position_norm(v))
                    .collect();
                let raw = (0..sm.positions.vert_count)
                    .filter_map(|v| sm.positions.position_raw(v))
                    .collect();
                let idx = sm
                    .indices
                    .as_ref()
                    .and_then(|s| s.indices())
                    .unwrap_or_default();
                *out = Some((pos, raw, idx));
            }
            *i += 1;
        }
        for k in c.children() {
            walk(k, i, tg, out);
        }
    }
    let mut out = None;
    let mut i = 0;
    walk(root, &mut i, target, &mut out);
    out
}

/// Removes the `target`-th `MESH` (pre-order, per [`flat_mesh_index`]) from its
/// parent and decrements that container's mesh count (info row's first BE u32).
/// Returns whether it removed.
fn delete_mesh(root: &mut Chunk, target: usize) -> bool {
    fn rec(c: &mut Chunk, i: &mut usize, target: usize) -> bool {
        let mut remove_at = None;
        {
            let Some(kids) = c.children_vec_mut() else {
                return false;
            };
            let mut ci = 0;
            while ci < kids.len() {
                if kids[ci].magic() == *b"MESH" && kids[ci].submesh().is_some() {
                    if *i == target {
                        remove_at = Some(ci);
                        break;
                    }
                    *i += 1;
                    ci += 1;
                } else if rec(&mut kids[ci], i, target) {
                    return true;
                } else {
                    ci += 1;
                }
            }
        }
        if let Some(ci) = remove_at {
            // this MDL directly held the target MESH: drop it and decrement the mesh
            // count cached at BE u32 +0x14 in the MDL's `HEAD` child
            let kids = c.children_vec_mut().unwrap();
            kids.remove(ci);
            for k in kids.iter_mut() {
                if k.magic() == *b"HEAD" {
                    if let Some(h) = k.leaf_mut()
                        && h.len() >= 0x18
                    {
                        let n = BE::read_u32(&h[0x14..0x18]);
                        if n > 0 {
                            BE::write_u32(&mut h[0x14..0x18], n - 1);
                        }
                    }
                    break;
                }
            }
            // The MDL's info row also caches the mesh count (first BE u32).
            if let Some(info) = c.info_mut() {
                let n = BE::read_u32(&info[0..4]);
                if n > 0 {
                    BE::write_u32(&mut info[0..4], n - 1);
                }
            }
            return true;
        }
        false
    }
    let mut i = 0;
    rec(root, &mut i, target)
}

/// Every op's `mesh` argument counts only these, skipping non-32-stride formats.
fn is_render_mesh(c: &Chunk) -> bool {
    c.magic() == *b"MESH" && c.submesh().is_some_and(|s| s.positions.stride == 32)
}

/// Clones the `target`-th renderable `MESH` chunk (pre-order) from a parsed WRB.
fn clone_nth_mesh(root: &Chunk, target: usize) -> Option<Chunk> {
    fn rec(c: &Chunk, i: &mut usize, target: usize) -> Option<Chunk> {
        if is_render_mesh(c) {
            if *i == target {
                return Some(c.clone());
            }
            *i += 1;
            return None;
        }
        for k in c.children() {
            if let Some(m) = rec(k, i, target) {
                return Some(m);
            }
        }
        None
    }
    let mut i = 0;
    rec(root, &mut i, target)
}

/// Inserts `mesh` into MDL `dst_model` at mesh-position `dst_mesh` (counting only
/// `MESH` children) and increments that MDL's two cached mesh counts (`HEAD` BE u32
/// @0x14 and info-row first BE u32). Inverse of [`delete_mesh`] for `:I`.
fn insert_mesh(root: &mut Chunk, dst_model: usize, dst_mesh: usize, mesh: Chunk) -> bool {
    fn nth_mdl<'a>(c: &'a mut Chunk, want: usize, i: &mut usize) -> Option<&'a mut Chunk> {
        if c.magic() == *b"MDL\0" || c.magic() == *b"MDL " {
            if *i == want {
                return Some(c);
            }
            *i += 1;
            return None;
        }
        for k in c.children_mut() {
            if let Some(f) = nth_mdl(k, want, i) {
                return Some(f);
            }
        }
        None
    }
    let mut i = 0;
    let Some(mdl) = nth_mdl(root, dst_model, &mut i) else {
        return false;
    };
    let Some(kids) = mdl.children_vec_mut() else {
        return false;
    };
    let mut seen = 0;
    let mut at = kids.len();
    for (ci, k) in kids.iter().enumerate() {
        if k.magic() == *b"MESH" && k.submesh().is_some() {
            if seen == dst_mesh {
                at = ci;
                break;
            }
            seen += 1;
        }
    }
    kids.insert(at, mesh);
    // An inserted mesh's HEAD byte +7 resets to 0; the clone carries its own value.
    for k in kids[at].children_vec_mut().into_iter().flatten() {
        if k.magic() == *b"HEAD" {
            if let Some(h) = k.leaf_mut()
                && h.len() > 7
            {
                h[7] = 0;
            }
            break;
        }
    }
    for k in kids.iter_mut() {
        if k.magic() == *b"HEAD" {
            if let Some(h) = k.leaf_mut()
                && h.len() >= 0x18
            {
                let n = BE::read_u32(&h[0x14..0x18]);
                BE::write_u32(&mut h[0x14..0x18], n + 1);
            }
            break;
        }
    }
    if let Some(info) = mdl.info_mut() {
        let n = BE::read_u32(&info[0..4]);
        BE::write_u32(&mut info[0..4], n + 1);
    }
    true
}

fn nth_mesh_mut<'a>(c: &'a mut Chunk, target: usize, i: &mut usize) -> Option<&'a mut Chunk> {
    if c.magic() == *b"MESH" && c.submesh().is_some() {
        if *i == target {
            return Some(c);
        }
        *i += 1;
        return None;
    }
    for k in c.children_mut() {
        if let Some(f) = nth_mesh_mut(k, target, i) {
            return Some(f);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cv(pos: [f32; 3], morph: [f32; 3]) -> ControlVertex {
        ControlVertex { pos, morph }
    }

    // An edge only splits when BOTH endpoints are marked corners, so the bundle needs controls at
    // the 3 vertices too, not just the midpoints.
    #[test]
    fn one_to_four_split() {
        let pos = vec![[-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [0.0, 0.5, 0.0]];
        let raw: Vec<[i16; 3]> = pos.iter().map(|&p| encode_pos(p)).collect();
        let idx = vec![0u16, 1, 2];
        let m = |a: [f32; 3], b: [f32; 3]| {
            [
                (a[0] + b[0]) / 2.0,
                (a[1] + b[1]) / 2.0,
                (a[2] + b[2]) / 2.0,
            ]
        };
        let controls = vec![
            cv(pos[0], pos[0]),
            cv(pos[1], pos[1]),
            cv(pos[2], pos[2]),
            cv(m(pos[0], pos[1]), m(pos[0], pos[1])),
            cv(m(pos[1], pos[2]), m(pos[1], pos[2])),
            cv(m(pos[2], pos[0]), m(pos[2], pos[0])),
        ];
        let s = subdivide(&pos, &raw, &idx, &controls, 0.001, None, &[]);
        assert_eq!(s.new_positions.len(), 3, "three edge midpoints");
        assert_eq!(s.indices.len(), 4, "1-to-4");
        // every triangle references valid verts; the center uses all 3 new ones
        let has_center = s
            .indices
            .iter()
            .any(|t| t.iter().all(|&v| v as usize >= pos.len()));
        assert!(has_center, "a center triangle of 3 midpoints");
    }

    // midpoint whose morph snaps onto an existing vertex must NOT be appended
    #[test]
    fn merge_suppresses_split() {
        let pos = vec![[-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [0.0, 0.5, 0.0]];
        let raw: Vec<[i16; 3]> = pos.iter().map(|&p| encode_pos(p)).collect();
        let idx = vec![0u16, 1, 2];
        // control at edge 0-1 midpoint, but morph == vertex 0
        let controls = vec![cv([0.0, -0.5, 0.0], pos[0])];
        let s = subdivide(&pos, &raw, &idx, &controls, 0.005, None, &[]);
        assert!(s.new_positions.is_empty(), "merged morph emits no vertex");
        assert_eq!(s.indices, vec![[0, 1, 2]], "triangle kept unsplit");
    }

    // Two edges split -> 3 tris. Under both-endpoints-marked, a 2-edge split happens
    // when all 3 verts are corners but one short edge's midpoint dedups onto an
    // endpoint (within merge_eps/2): edge 2-0 is short, so only 0-1 and 1-2 survive.
    #[test]
    fn two_edge_split_three_tris() {
        let pos = vec![[-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [-0.48, -0.46, 0.0]];
        let raw: Vec<[i16; 3]> = pos.iter().map(|&p| encode_pos(p)).collect();
        let idx = vec![0u16, 1, 2];
        let controls = vec![cv(pos[0], pos[0]), cv(pos[1], pos[1]), cv(pos[2], pos[2])];
        let s = subdivide(&pos, &raw, &idx, &controls, 0.07, None, &[]);
        assert_eq!(s.new_positions.len(), 2);
        assert_eq!(s.indices.len(), 3);
    }

    // End-to-end `:~` on a real model (set FF13_TILDE_TRB to a c001 `.trb`,
    // FF13_TILDE_BUNDLE to the bundle): applies Armband (:~,1,5), checks the output
    // re-parses with grown geometry.
    #[test]
    fn apply_op_armband_roundtrips() {
        let (Ok(trb_path), Ok(bundle_path)) = (
            std::env::var("FF13_TILDE_TRB"),
            std::env::var("FF13_TILDE_BUNDLE"),
        ) else {
            eprintln!("skipping: set FF13_TILDE_TRB + FF13_TILDE_BUNDLE");
            return;
        };
        let trb = std::fs::read(trb_path).unwrap();
        let bundle = std::fs::read(bundle_path).unwrap();
        let out = apply_op(&trb, &bundle, "X_c001_Armband", 1, 5).expect("apply_op");
        assert!(out.len() > trb.len(), "output grew");
        // re-parse: submesh 9 has +268 verts, 9159 indices
        use ff13_formats::trb::Trb;
        use ff13_formats::wrb;
        let t = Trb::parse(&out).unwrap();
        let res = t
            .resource_data(t.find_resource(b"SEDBwrb").unwrap())
            .unwrap();
        let root = wrb::parse(res).unwrap();
        let (pos, _, idx) = read_submesh(&root, 9).unwrap();
        assert_eq!(pos.len(), 3232, "submesh 9 vert count");
        assert_eq!(idx.len(), 9159, "submesh 9 index count");
        if let Ok(engine_path) = std::env::var("FF13_TILDE_ENGINE") {
            let engine = std::fs::read(engine_path).unwrap();
            assert_eq!(
                out, engine,
                "output must be byte-identical to FFXIII_ModelsHD.exe"
            );
        }
    }
}
