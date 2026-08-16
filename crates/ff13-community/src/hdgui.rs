//! Bakes the HD Fonts and GUI pack into a LayeredFS mods tree, matched by content hash.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ff13_formats::imgb;

// FNV-1a-64 over mip-0: the key both the pack DB and the proxy swap use.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Outcome of a hash-resolved emit. Names are the pack's `category/name` keys.
#[derive(Default)]
pub struct HashReport {
    /// Placed at a resolved game path+index.
    pub written: Vec<String>,
    /// Hash not found among scanned game textures.
    pub unmatched: Vec<String>,
    /// Hash matched, but the pack's `.dds` is missing.
    pub missing_dds: Vec<String>,
}

/// Parses `hash_database.txt` (`<16-hex> <category/name>` lines, `#` comments)
/// into `hash -> name`.
fn parse_hash_db(path: &Path) -> Result<HashMap<u64, String>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(parse_hash_db_text(&text))
}

/// Parses the DB body: `<16-hex> <category/name>` lines; `#` = comment, blank/
/// short/non-hex skipped.
fn parse_hash_db_text(text: &str) -> HashMap<u64, String> {
    let mut db = HashMap::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut it = line.split_whitespace();
        if let (Some(h), Some(name)) = (it.next(), it.next())
            && let Ok(h) = u64::from_str_radix(h, 16)
        {
            db.insert(h, name.to_string());
        }
    }
    db
}

/// A scanned container: `.imgb` pixel blob + header (`.win32.xgr` WPD or bare
/// `.gtex`), or `None` for header-less scene maps (whole `.imgb` = one mip).
struct Scanned {
    header: Option<PathBuf>,
    imgb: PathBuf,
}

/// All texture containers under `root` (excluding models: `.imgb` with a `.trb`
/// sibling), each `.imgb` paired with its `.xgr`/`.gtex` header if present.
fn walk_containers(root: &Path) -> Vec<Scanned> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
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
            let Some(ibase) = p
                .file_name()
                .and_then(|f| f.to_str())
                .and_then(|n| n.strip_suffix(".imgb"))
            else {
                continue;
            };
            let sib = |ext: &str| p.with_file_name(format!("{ibase}.{ext}"));
            if sib("xgr").exists() {
                out.push(Scanned {
                    header: Some(sib("xgr")),
                    imgb: p,
                });
            } else if sib("gtex").exists() {
                out.push(Scanned {
                    header: Some(sib("gtex")),
                    imgb: p,
                });
            } else if !sib("trb").exists() {
                out.push(Scanned {
                    header: None,
                    imgb: p,
                }); // bare scene-map imgb
            }
        }
    }
    out
}

/// Resolves the pack to game paths by content hash and emits a LayeredFS
/// tree. Scans containers under `scan_root` (`.xgr`/`.gtex`/bare `.imgb`), hashes
/// each mip-0, and where the pack's `hash_database.txt` (FNV-1a(mip0)->name) has
/// it, copies the `.dds` to `<imgb-base>.<index>.dds`. Proxy re-derives the hash
/// at runtime: no DB consulted at runtime.
pub fn emit_layeredfs_hashed(
    hd_dir: &Path,
    white_data: &Path,
    scan_root: &Path,
    mods_out: &Path,
    dry_run: bool,
) -> Result<HashReport> {
    let db = parse_hash_db(&hd_dir.join("hash_database.txt"))?;
    let mut report = HashReport::default();
    let mut matched: HashSet<u64> = HashSet::new();
    for c in walk_containers(scan_root) {
        let Ok(imgb) = std::fs::read(&c.imgb) else {
            continue;
        };
        // (texture index, mip-0 hash) per texture in the container
        let textures: Vec<(usize, u64)> = match &c.header {
            Some(h) => {
                let Ok(hdr) = std::fs::read(h) else { continue };
                imgb::valid_gtex(&hdr, imgb.len())
                    .iter()
                    .enumerate()
                    .filter_map(|(i, g)| {
                        let &(s, sz) = g.mips.first()?;
                        let slice = imgb.get(s as usize..s as usize + sz as usize)?;
                        Some((i, fnv1a64(slice)))
                    })
                    .collect()
            }
            None => vec![(0, fnv1a64(&imgb))], // bare imgb = single mip
        };
        let ibase = c
            .imgb
            .file_name()
            .and_then(|f| f.to_str())
            .and_then(|n| n.strip_suffix(".imgb"))
            .unwrap_or("tex");
        let rel = c.imgb.strip_prefix(white_data).unwrap_or(&c.imgb);
        let dir = rel.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        for (i, h) in textures {
            let Some(name) = db.get(&h) else { continue };
            matched.insert(h);
            let pack_dds = hd_dir.join(format!("{name}.dds"));
            if !pack_dds.exists() {
                report.missing_dds.push(name.clone());
                continue;
            }
            let dest = mods_out.join(&dir).join(format!("{ibase}.{i}.dds"));
            if !dry_run {
                if let Some(p) = dest.parent() {
                    std::fs::create_dir_all(p)?;
                }
                std::fs::copy(&pack_dds, &dest)
                    .with_context(|| format!("writing {}", dest.display()))?;
            }
            report.written.push(name.clone());
        }
    }
    for (h, name) in &db {
        if !matched.contains(h) {
            report.unmatched.push(name.clone());
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a64_matches_standard_vectors() {
        // The pack DB and the proxy swap both key on this, so any drift breaks every match.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171_f73967e8);
    }

    #[test]
    fn parse_hash_db_text_handles_comments_blanks_and_bad_hex() {
        let txt = "\
# GENERAL GUI section
0a4cba213a95c4c9 gui_resident/atb_uv
c8e02ba2e4a784a5 gui_resident/aura_uv   # trailing comment

zzzz not_hex_skip_me
69c97d82fae042ee map_scene00023/scene00023_01_split_00
";
        let db = parse_hash_db_text(txt);
        assert_eq!(db.len(), 3, "comment, blank and non-hex lines are skipped");
        assert_eq!(
            db.get(&0x0a4cba213a95c4c9).map(String::as_str),
            Some("gui_resident/atb_uv")
        );
        assert_eq!(
            db.get(&0xc8e02ba2e4a784a5).map(String::as_str),
            Some("gui_resident/aura_uv")
        );
        assert_eq!(
            db.get(&0x69c97d82fae042ee).map(String::as_str),
            Some("map_scene00023/scene00023_01_split_00")
        );
    }
}
