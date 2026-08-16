//! `SEDBSKL` skeleton merge: the `:O`/`:E`/`:X` section of an HD-Models edit script.

use std::collections::HashMap;

const HDR: usize = 0x50;
const REC: usize = 176;

/// The hardcoded list `:F` "Face Fix" uses; each shared face bone takes the SOURCE transform.
/// Matched case-insensitively. `L_broln`/`R_broln` are typos for `L_broIn`/`R_broIn` that therefore
/// never match, leaving brow-inner bones on BASE; kept verbatim so output stays byte-exact.
const FACE_BONES: [&str; 31] = [
    "C_U", "C_Jaw", "C_throat", "C_wrin", "L_Dlid", "L_U", "L_Ulid", "L_brow", "L_cheek", "L_cor",
    "L_eye", "L_nose", "L_puff", "R_Dlid", "R_U", "R_Ulid", "R_brow", "R_cheek", "R_cor", "R_eye",
    "R_nose", "R_puff", "C_D", "C_chin", "C_tong", "L_D", "L_broln", "R_D", "R_broln", "L_wrin",
    "R_wrin",
];

fn is_face_bone(name: &str) -> bool {
    FACE_BONES.iter().any(|f| f.eq_ignore_ascii_case(name))
}

/// `:E,a,b,c` field selector -> byte offset within a 176-byte record.
/// `b6..=b15` are f32, the rest i32.
pub const E_OFFSET: [usize; 17] = [
    0x04, 0x00, 0x38, 0x3c, 0x40, 0x4c, 0x10, 0x14, 0x18, 0x1c, 0x20, 0x24, 0x28, 0x2c, 0x30, 0x34,
    0x44,
];

struct Skel {
    hdr: Vec<u8>,
    rec: Vec<Vec<u8>>,
    tok: Vec<String>,
}

fn gi(rec: &[u8], off: usize) -> i32 {
    i32::from_le_bytes(rec[off..off + 4].try_into().unwrap())
}
fn si(rec: &mut [u8], off: usize, v: i32) {
    rec[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

impl Skel {
    fn parse(b: &[u8]) -> Option<Skel> {
        if b.len() < HDR || &b[..7] != b"SEDBSKL" {
            return None;
        }
        let cnt = u32::from_le_bytes(b[0x34..0x38].try_into().ok()?) as usize / REC;
        let mut rec = Vec::with_capacity(cnt);
        for i in 0..cnt {
            let o = HDR + i * REC;
            rec.push(b[o..o + REC].to_vec());
        }
        let tok = b[HDR + cnt * REC..]
            .split(|&c| c == 0)
            .map(|t| String::from_utf8_lossy(t).into_owned())
            .collect();
        Some(Skel {
            hdr: b[..HDR].to_vec(),
            rec,
            tok,
        })
    }

    fn name(&self, i: usize) -> &str {
        let k = gi(&self.rec[i], 0);
        self.tok.get(k as usize).map(String::as_str).unwrap_or("?")
    }
}

/// `:S` op: rewrite the hierarchy into a flat chain, each bone's first-child being the next index
/// and every sibling link cleared.
pub fn apply_s(skl: &mut [u8]) {
    if skl.len() < HDR + 4 {
        return;
    }
    let cnt = u32::from_le_bytes(skl[0x34..0x38].try_into().unwrap()) as usize / REC;
    for i in 0..cnt {
        let rec = HDR + i * REC;
        let next = if i + 1 < cnt { (i + 1) as i32 } else { -1 };
        si(&mut skl[rec..], 0x3c, next);
        si(&mut skl[rec..], 0x40, -1);
    }
}

/// The bone names of a `SEDBSKL` blob, in record order.
pub fn bone_names(skl: &[u8]) -> Option<Vec<String>> {
    let s = Skel::parse(skl)?;
    Some((0..s.rec.len()).map(|i| s.name(i).to_string()).collect())
}

/// Intrinsic to the merge: on `fing` bones whose digit is 3/4/5, shift the trailing chain letter
/// one along, so `L_fing3a` becomes `L_fing3b`.
fn finger(name: &str) -> String {
    let Some(i) = name.find("fing") else {
        return name.to_string();
    };
    let bytes = name.as_bytes();
    if i + 5 >= bytes.len() {
        return name.to_string();
    }
    let digit = bytes[i + 4];
    if !matches!(digit, b'3' | b'4' | b'5') {
        return name.to_string();
    }
    let next = match bytes[i + 5] {
        b'a' => b'b',
        b'b' => b'c',
        _ => return name.to_string(),
    };
    let mut out = name.as_bytes().to_vec();
    out[i + 5] = next;
    String::from_utf8(out).unwrap()
}

/// A source bone's final merged name: rename pairs applied (substring when
/// `exact` false: `+N` rename-in-place, so `R_hair1b`->`R_dmhr1b` also rewrites
/// `UR_hair1b`; whole-name when `exact` true: `-N` add-copies), then (if
/// `finger_shift`) the finger-chain rename.
pub fn renamed_name(
    name: &str,
    renames: &[(String, String)],
    finger_shift: bool,
    exact: bool,
) -> String {
    let mut n = name.to_string();
    for (search, replace) in renames {
        if search.is_empty() {
            continue;
        }
        if exact {
            if n == *search {
                n = replace.clone();
                break;
            }
        } else {
            n = n.replace(search.as_str(), replace);
        }
    }
    if finger_shift { finger(&n) } else { n }
}

/// Whether a skeleton has 3-joint fingers (a `fing{3,4,5}c` bone); the finger
/// shift (`b->c`) only fires when the donor/base rig has these.
pub fn has_three_joint_fingers(skl: &[u8]) -> bool {
    bone_names(skl).is_some_and(|ns| {
        ns.iter().any(|n| {
            let b = n.as_bytes();
            b.windows(5)
                .any(|w| &w[..4] == b"fing" && matches!(w[4], b'3' | b'4' | b'5'))
                && n.ends_with('c')
        })
    })
}

/// Source bones whose name changes under the merge as `(orig, renamed)` pairs,
/// for renaming bone *references* in other resources (e.g. SEDBPHB physics).
pub fn changed_bones(
    src_skl: &[u8],
    renames: &[(String, String)],
    finger_shift: bool,
    exact: bool,
) -> Vec<(String, String)> {
    let Some(src) = Skel::parse(src_skl) else {
        return Vec::new();
    };
    (0..src.rec.len())
        .filter_map(|i| {
            let o = src.name(i).to_string();
            let r = renamed_name(&o, renames, finger_shift, exact);
            (r != o).then_some((o, r))
        })
        .collect()
}

/// Rename whole NUL-delimited bone-name tokens in a blob, in place (same length).
/// Only standalone tokens, never labels like `pin_R_hair1b_001` that merely contain a name.
pub fn rename_tokens(blob: &mut [u8], changed: &[(String, String)]) {
    for (o, r) in changed {
        let (ob, rb) = (o.as_bytes(), r.as_bytes());
        if ob.len() != rb.len() || ob.is_empty() {
            continue;
        }
        let (n, l) = (blob.len(), ob.len());
        let mut i = 0;
        while i + l <= n {
            let bounded = (i == 0 || blob[i - 1] == 0) && (i + l == n || blob[i + l] == 0);
            if bounded && &blob[i..i + l] == ob {
                blob[i..i + l].copy_from_slice(rb);
                i += l;
            } else {
                i += 1;
            }
        }
    }
}

/// `renames` are the header's `(search, replace)` pairs, matched as substrings unless `exact`;
/// the finger-chain rename is added automatically. `e_ops` are parsed `:E,a,b,c` ops.
pub fn merge(
    base_skl: &[u8],
    src_skl: &[u8],
    renames: &[(String, String)],
    e_ops: &[(usize, u8, i32)],
    face_fix: bool,
    exact: bool,
) -> Option<Vec<u8>> {
    let base = Skel::parse(base_skl)?;
    let src = Skel::parse(src_skl)?;
    // Finger shift (a->b, b->c) grafts a 2-joint SOURCE rig's fingers onto a 3-joint
    // donor (BASE). Fires only when BASE is 3-joint AND source is NOT. Else both
    // source `fingNb`/`fingNc` map onto `fingNc`, duplicating the fingertip.
    let finger_shift = has_three_joint_fingers(base_skl) && !has_three_joint_fingers(src_skl);
    let sname = |i: usize| -> String { renamed_name(src.name(i), renames, finger_shift, exact) };

    // 1. Order: source (renamed), then base-only bones. Matching is
    // CASE-INSENSITIVE: a source bone matching a base bone only by case (source
    // `L_hair1a` vs base `L_hair1A`) is the SAME bone, adopts the BASE name/casing
    // (uses BASE record in step 2), base copy not re-appended.
    let base_canon: HashMap<String, &str> = (0..base.rec.len())
        .map(|i| (base.name(i).to_ascii_lowercase(), base.name(i)))
        .collect();
    let mut order: Vec<(bool, usize, String)> = Vec::new(); // (is_src_entry, orig_idx, final_name)
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for i in 0..src.rec.len() {
        let raw = sname(i);
        let fn_ = base_canon
            .get(&raw.to_ascii_lowercase())
            .map(|s| s.to_string())
            .unwrap_or(raw);
        seen.insert(fn_.to_ascii_lowercase());
        order.push((true, i, fn_));
    }
    for i in 0..base.rec.len() {
        let n = base.name(i).to_string();
        if seen.insert(n.to_ascii_lowercase()) {
            order.push((false, i, n));
        }
    }
    let cnt = order.len();
    let name_pos: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(p, (_, _, n))| (n.as_str(), p))
        .collect();
    let src_pos: HashMap<usize, usize> = order
        .iter()
        .enumerate()
        .filter(|(_, (s, _, _))| *s)
        .map(|(p, (_, o, _))| (*o, p))
        .collect();
    let base_idx: HashMap<&str, usize> = (0..base.rec.len()).map(|i| (base.name(i), i)).collect();

    let remap_base = |t: i32| -> i32 {
        if t < 0 {
            return t;
        }
        name_pos
            .get(base.name(t as usize))
            .map(|&p| p as i32)
            .unwrap_or(-1)
    };
    let remap_src = |t: i32| -> i32 {
        if t < 0 {
            return t;
        }
        src_pos.get(&(t as usize)).map(|&p| p as i32).unwrap_or(-1)
    };

    // 2. Build records (shared/base-only: BASE record; src-only/renamed: SRC).
    let mut rec: Vec<Vec<u8>> = Vec::with_capacity(cnt);
    let mut is_new = vec![false; cnt];
    for (p, (is_src, oi, fn_)) in order.iter().enumerate() {
        let mut r = if let Some(&bi) = base_idx.get(fn_.as_str()) {
            let mut r = base.rec[bi].clone();
            for off in [0x38, 0x3c, 0x40] {
                let v = remap_base(gi(&r, off));
                si(&mut r, off, v);
            }
            // `:F` Face Fix: a shared face bone ([`FACE_BONES`]) takes transform
            // [+0x10,+0x34) from SOURCE (target's face pose), keeping links from
            // BASE. Body bones (and all bones with no `:F`) keep BASE's pose.
            if face_fix && *is_src && is_face_bone(fn_) {
                r[0x10..0x34].copy_from_slice(&src.rec[*oi][0x10..0x34]);
            }
            r
        } else {
            is_new[p] = true;
            let mut r = src.rec[*oi].clone();
            for off in [0x38, 0x3c, 0x40] {
                let v = remap_src(gi(&r, off));
                si(&mut r, off, v);
            }
            r
        };
        // 3. Flush near-zero transform floats.
        let mut off = 0x10;
        while off < 0x34 {
            let f = f32::from_le_bytes(r[off..off + 4].try_into().unwrap());
            if f != 0.0 && f.abs() < 1e-5 {
                r[off..off + 4].copy_from_slice(&0f32.to_le_bytes());
            }
            off += 4;
        }
        si(&mut r, 0x44, p as i32);
        si(&mut r, 0x48, 0x12);
        rec.push(r);
    }

    // 4. `:E` ops.
    for &(a, b, c) in e_ops {
        if a < cnt
            && let Some(&off) = E_OFFSET.get(b as usize)
        {
            si(&mut rec[a], off, c);
        }
    }

    // 5. AddMissingBones artifact: chain new bones through +0x3c.
    let new_pos: Vec<usize> = (0..cnt).filter(|&p| is_new[p]).collect();
    for (j, &p) in new_pos.iter().enumerate() {
        let next = new_pos.get(j + 1).map(|&n| n as i32).unwrap_or(-1);
        si(&mut rec[p], 0x3c, next);
        si(&mut rec[p], 0x40, -1);
        si(&mut rec[p], 0x4c, 2);
    }

    // 6. Token pool: insert new names before "HierarchyData", in source order.
    let hd_name = "HierarchyData";
    let mut tok = base.tok.clone();
    let mut hd = tok.iter().position(|t| t == hd_name)?;
    let base_set: std::collections::HashSet<&str> = base.tok.iter().map(String::as_str).collect();
    for (_, _, fn_) in &order {
        if !base_set.contains(fn_.as_str()) {
            tok.insert(hd, fn_.clone());
            hd += 1;
        }
    }
    let mut key: HashMap<&str, usize> = HashMap::new();
    for (i, t) in tok.iter().enumerate() {
        key.entry(t.as_str()).or_insert(i);
    }
    for (p, (_, _, fn_)) in order.iter().enumerate() {
        si(&mut rec[p], 0x00, key[fn_.as_str()] as i32);
    }
    let used = &tok[..hd + 1];
    let mut pool: Vec<u8> = Vec::new();
    for (i, t) in used.iter().enumerate() {
        if i > 0 {
            pool.push(0);
        }
        pool.extend_from_slice(t.as_bytes());
    }
    pool.push(0);
    while !pool.len().is_multiple_of(4) {
        pool.push(0);
    }

    // 7. Serialize.
    let mut out = base.hdr.clone();
    let total = (HDR + cnt * REC + pool.len()) as u32;
    si(&mut out, 0x10, total as i32);
    si(&mut out, 0x34, (cnt * REC) as i32);
    si(&mut out, 0x38, (cnt + 4) as i32);
    si(&mut out, 0x40, (cnt + 3) as i32);
    si(&mut out, 0x48, (cnt * REC) as i32);
    for r in &rec {
        out.extend_from_slice(r);
    }
    out.extend_from_slice(&pool);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_skl(trb: &[u8]) -> Vec<u8> {
        let rc = u32::from_le_bytes(trb[56..60].try_into().unwrap()) as usize;
        let ds = 64 + rc * 16;
        for i in 0..rc {
            let e = 64 + i * 16;
            let off = u32::from_le_bytes(trb[e + 4..e + 8].try_into().unwrap()) as usize;
            let sz = u32::from_le_bytes(trb[e + 8..e + 12].try_into().unwrap()) as usize;
            if &trb[ds + off..ds + off + 7] == b"SEDBSKL" {
                return trb[ds + off..ds + off + sz].to_vec();
            }
        }
        panic!("no SEDBSKL");
    }

    #[test]
    fn finger_rename() {
        assert_eq!(finger("L_fing3a"), "L_fing3b");
        assert_eq!(finger("L_fing5b"), "L_fing5c");
        assert_eq!(finger("L_fing1a"), "L_fing1a");
        assert_eq!(finger("L_femur"), "L_femur");
    }

    #[test]
    fn c201_merge_byte_exact() {
        let (base_p, src_p, eng_p) = (
            "/tmp/c201_noskel.trb",
            "/tmp/mhd_run/models/orig/chr/pc/c201/bin/c201.win32.trb",
            "/tmp/c201_engine.trb",
        );
        let all = [base_p, src_p, eng_p];
        if all.iter().any(|p| !std::path::Path::new(p).exists()) {
            eprintln!("skipping: c201 merge reference files absent");
            return;
        }
        let base = find_skl(&std::fs::read(base_p).unwrap());
        let src = find_skl(&std::fs::read(src_p).unwrap());
        let eng = find_skl(&std::fs::read(eng_p).unwrap());

        // 11 header pairs (`+11` rename-in-place -> substring)
        let renames: Vec<(String, String)> = [
            ("L_hair1a", "L_dmhr1a"),
            ("L_hair4a", "L_dmhr4a"),
            ("R_hair1b", "R_dmhr1b"),
            ("R_hair3a", "R_dmhr3a"),
            ("R_hair5a", "R_dmhr5a"),
            ("L_hair2a", "L_dmhr2a"),
            ("R_hair1a", "R_dmhr1a"),
            ("R_hair2a", "R_dmhr2a"),
            ("R_hair4a", "R_dmhr4a"),
            ("T_hair1a", "T_dmhr1a"),
            ("T_hair2a", "T_dmhr2a"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();

        let e_ops: Vec<(usize, u8, i32)> = vec![
            (4, 4, 9),
            (79, 4, -1),
            (10, 4, 11),
            (107, 4, -1),
            (11, 4, 12),
            (111, 4, -1),
            (13, 3, 16),
            (210, 4, -1),
            (15, 3, 25),
            (211, 4, -1),
            (36, 3, 37),
            (213, 4, -1),
            (50, 4, 51),
            (83, 4, -1),
            (51, 3, 52),
            (102, 4, -1),
            (53, 3, 88),
            (91, 4, -1),
            (59, 3, 93),
            (96, 4, -1),
            (79, 4, 108),
            (127, 4, -1),
            (114, 4, 122),
            (147, 4, -1),
            (33, 3, 134),
            (277, 3, -1),
            (42, 3, 43),
        ];

        // X_Lightning2's `:X` has no `:F`, so shared bones keep BASE transform
        let merged = merge(&base, &src, &renames, &e_ops, false, false).expect("merge");
        assert_eq!(merged.len(), eng.len(), "size mismatch");
        assert!(merged == eng, "skeleton merge not byte-identical");
    }

    /// Minimal `SEDBSKL`: one record per name (token idx = position, links -1) +
    /// NUL-terminated name pool.
    fn build_skel(names: &[&str]) -> Vec<u8> {
        let mut out = vec![0u8; HDR];
        out[..7].copy_from_slice(b"SEDBSKL");
        out[0x34..0x38].copy_from_slice(&((names.len() * REC) as u32).to_le_bytes());
        for (i, _) in names.iter().enumerate() {
            let mut r = vec![0u8; REC];
            r[0..4].copy_from_slice(&(i as i32).to_le_bytes()); // name token index
            for off in [0x38usize, 0x3c, 0x40] {
                r[off..off + 4].copy_from_slice(&(-1i32).to_le_bytes()); // no links
            }
            out.extend_from_slice(&r);
        }
        // bone-name tokens + trailing "HierarchyData" anchor (always present in a real SEDBSKL)
        for n in names.iter().chain(std::iter::once(&"HierarchyData")) {
            out.extend_from_slice(n.as_bytes());
            out.push(0);
        }
        out
    }

    #[test]
    fn merge_dedups_bones_case_insensitively() {
        // source matching base only by CASE (`l_hair1a` vs `L_hair1A`) is the SAME
        // bone, must NOT be appended twice
        let base = build_skel(&["root", "L_hair1A"]);
        let src = build_skel(&["root", "l_hair1a"]);
        let merged = merge(&base, &src, &[], &[], false, false).expect("merge");
        let names = bone_names(&merged).expect("names");
        assert_eq!(
            names,
            vec!["root".to_string(), "L_hair1A".to_string()],
            "case-only-differing source bone must dedup against base, not duplicate"
        );
    }
}
