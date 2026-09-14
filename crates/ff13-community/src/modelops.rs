//! Parsing HD-pack model scripts and building data-only model assets.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::modscript::Script;
use crate::{skel, tilde};
use ff13_formats::trb::Trb;

/// Strip a leading `zoneu\z###\` / `zonec\z###\` (the HD pack's duplicate-archive zone prefix) so the path maps to the real game tree.
fn strip_zone_prefix(p: &str) -> String {
    let parts: Vec<&str> = p.split('\\').collect();
    if parts.len() >= 2 {
        let (a, b) = (parts[0], parts[1]);
        if (a == "zoneu" || a == "zonec")
            && b.starts_with('z')
            && b.len() > 1
            && b[1..].chars().all(|c| c.is_ascii_digit())
        {
            return parts[2..].join("\\");
        }
    }
    p.to_string()
}

/// True for a zone-block `c`-region twin (`z###block###c.txt`): duplicate-archive copy of `u`, identical geometry, skipped.
fn is_zone_c_twin(name: &str) -> bool {
    name.starts_with('z') && name.contains("block") && name.ends_with("c.txt")
}

/// Read each source model's `.trb` + `.imgb` under `root` (paths data-dir
/// relative, no extension, `\` or `/` separators).
pub fn load_models(root: &Path, model_paths: &[&str]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
    model_paths
        .iter()
        .map(|mp| {
            let rel = |ext: &str| root.join(format!("{}.{ext}", mp.replace('\\', "/")));
            let trb =
                std::fs::read(rel("trb")).map_err(|e| anyhow!("source model {mp}.trb: {e}"))?;
            let imgb =
                std::fs::read(rel("imgb")).map_err(|e| anyhow!("source model {mp}.imgb: {e}"))?;
            Ok((trb, imgb))
        })
        .collect()
}

/// Index of the source whose slot the output extends (its path prefixes the
/// output `.trb` path), defaulting to the last source.
fn base_index(model_paths: &[&str], out_rel: &str) -> Result<usize> {
    model_paths
        .iter()
        .position(|mp| out_rel.starts_with(*mp))
        .or_else(|| model_paths.len().checked_sub(1))
        .ok_or_else(|| anyhow!("script lists no source models (.win32 lines)"))
}

/// The base model's skeleton resource plus the map slots that take a physics
/// (`SEDBPHB`) resource from it (those need bone-rename fixups after a merge).
pub fn skeleton_and_phb(
    trbs: &[Trb],
    map: &[(usize, usize)],
    base_idx: usize,
) -> (Option<Vec<u8>>, Vec<usize>) {
    let target = &trbs[base_idx];
    let src_skl = target
        .find_resource(b"SEDBSKL")
        .and_then(|i| target.resource_data(i))
        .map(<[u8]>::to_vec);
    let target_phb = map
        .iter()
        .enumerate()
        .filter(|&(_, &(m, r))| {
            m == base_idx
                && trbs[m]
                    .resource_data(r)
                    .is_some_and(|d| d.starts_with(b"SEDBPHB"))
        })
        .map(|(i, _)| i)
        .collect();
    (src_skl, target_phb)
}

/// Fold `src_skl` (the base model's rig) into `result`'s skeleton resource.
#[allow(clippy::too_many_arguments)]
pub fn merge_skeleton(
    result: &[u8],
    src_skl: &[u8],
    renames: &[(String, String)],
    e_ops: &[(usize, u8, i32)],
    face_fix: bool,
    rename_exact: bool,
    skel_s: bool,
) -> Result<Vec<u8>> {
    let trb = Trb::parse(result)?;
    let skl_idx = trb
        .find_resource(b"SEDBSKL")
        .ok_or_else(|| anyhow!("no SEDBSKL resource to merge"))?;
    let base_skl = trb.resource_data(skl_idx).unwrap();
    let mut merged = skel::merge(base_skl, src_skl, renames, e_ops, face_fix, rename_exact)
        .ok_or_else(|| anyhow!("skeleton merge failed"))?;
    if skel_s {
        skel::apply_s(&mut merged);
    }
    Ok(trb.serialize_replacing(skl_idx, &merged)?)
}

/// Rename target physics bone refs (hair->dmhr, …) to follow the merged rig; donor physics keep their names.
pub fn rename_phb_bones(
    result: &mut [u8],
    src_skl: &[u8],
    renames: &[(String, String)],
    rename_exact: bool,
    target_phb: &[usize],
) -> Result<()> {
    let (changed, spans) = {
        let trb = Trb::parse(result)?;
        let finger_shift = trb
            .find_resource(b"SEDBSKL")
            .and_then(|i| trb.resource_data(i))
            .is_some_and(skel::has_three_joint_fingers);
        let changed = skel::changed_bones(src_skl, renames, finger_shift, rename_exact);
        if changed.is_empty() || target_phb.is_empty() {
            return Ok(());
        }
        let spans: Vec<(usize, usize)> = target_phb
            .iter()
            .filter_map(|&i| trb.resource_abs_span(i))
            .collect();
        (changed, spans)
    };
    for (a, b) in spans {
        skel::rename_tokens(&mut result[a..b], &changed);
    }
    Ok(())
}

pub fn pad16(imgb: &mut Vec<u8>) {
    while !imgb.len().is_multiple_of(16) {
        imgb.push(0);
    }
}

/// The model an outfit-swap script builds: the script's output `.trb` path
/// (data-dir relative, backslash separated as written), the combined container
/// bytes, and how many source models fed the combine.
#[derive(Debug)]
pub struct SwapBuild {
    pub out_rel: String,
    pub trb: Vec<u8>,
    pub imgb: Vec<u8>,
    pub sources: usize,
}

/// Build an outfit-swap model from an HD-Models swap script (a `.txt` with a
/// resource map): combines the script's source models per its header map at
/// native resolution. Geometry/textures/materials/names are reproduced; the
/// skeleton stays the mesh source's rig.
pub fn build_outfit_swap(script: &Path, white_data: &Path) -> Result<SwapBuild> {
    let text = std::fs::read_to_string(script)?.replace("\r\n", "\n");
    let s = Script::parse(&text);
    let map = s.combine_map().ok_or_else(|| {
        anyhow!(
            "{} has no resource map: not an outfit-swap script",
            script.display()
        )
    })?;
    let renames = s.rename_pairs();
    let model_paths = s.model_paths();
    let out_rel = s
        .output_paths()
        .0
        .ok_or_else(|| anyhow!("script has no output .trb path"))?
        .to_string();

    let loaded = load_models(white_data, &model_paths)?;
    let trbs: Vec<Trb> = loaded
        .iter()
        .map(|(t, _)| Trb::parse(t))
        .collect::<Result<_, _>>()?;
    let sources: Vec<(&Trb, &[u8])> = trbs
        .iter()
        .zip(&loaded)
        .map(|(t, (_, i))| (t, i.as_slice()))
        .collect();

    let base_idx = base_index(&model_paths, &out_rel)?;
    let (trb, imgb) = Trb::combine_model(&trbs[base_idx], &sources, &map, &renames)
        .ok_or_else(|| anyhow!("combine failed (unexpected model layout)"))?;
    Ok(SwapBuild {
        out_rel,
        trb,
        imgb,
        sources: model_paths.len(),
    })
}

/// Runs the whole pipeline in order: combine, `:~` and edit ops, `:O` skeleton merge, then the
/// physics bone-rename, writing the resulting `.trb` and `.imgb` to `out`. `bundle` is the pack's
/// model bundle `.bin`; `orig` is the source-model root, required by combine
/// scripts.
pub fn run_model_script(
    model: &Path,
    script: &Path,
    bundle: &Path,
    out: &Path,
    orig: Option<&Path>,
) -> Result<()> {
    let bundle_data = std::fs::read(bundle)?;
    run_model_script_bytes(model, script, &bundle_data, out, orig)
}

fn run_model_script_bytes(
    model: &Path,
    script: &Path,
    bundle_data: &[u8],
    out: &Path,
    orig: Option<&Path>,
) -> Result<()> {
    let text = std::fs::read_to_string(script)?.replace("\r\n", "\n");
    let s = Script::parse(&text);

    #[allow(clippy::type_complexity)]
    let (base_trb, src_skl, target_phb, mut imgb, source_trbs): (
        Vec<u8>,
        Option<Vec<u8>>,
        Vec<usize>,
        Vec<u8>,
        Vec<Vec<u8>>,
    ) = match (s.combine_map(), orig) {
        (Some(map), Some(root)) => {
            let renames = s.rename_pairs();
            let model_paths = s.model_paths();
            let out_rel = s
                .output_paths()
                .0
                .ok_or_else(|| anyhow!("script has no output .trb path"))?
                .to_string();
            let loaded = load_models(root, &model_paths)?;
            let trbs: Vec<Trb> = loaded
                .iter()
                .map(|(t, _)| Trb::parse(t))
                .collect::<Result<_, _>>()?;
            let sources: Vec<(&Trb, &[u8])> = trbs
                .iter()
                .zip(&loaded)
                .map(|(t, (_, i))| (t, i.as_slice()))
                .collect();
            let base_idx = base_index(&model_paths, &out_rel)?;
            println!(
                "Combine: {} source model(s), {} resource-map entries",
                model_paths.len(),
                map.len()
            );
            let (src_skl, target_phb) = skeleton_and_phb(&trbs, &map, base_idx);
            let source_trbs: Vec<Vec<u8>> = loaded.iter().map(|(t, _)| t.clone()).collect();
            // Single-source IDENTITY map (output[i] = source(0, i)) is a no-op combine. Some packs
            // (e.g. FF XIII HD's c005) omit the trailing RESOURCE_TYPE/RESOURCE_ID that `combine_model`
            // needs, so it returns None. Run ops on the source directly instead.
            let identity = model_paths.len() == 1
                && map.iter().enumerate().all(|(i, &(m, r))| m == 0 && r == i);
            match Trb::combine_model(&trbs[base_idx], &sources, &map, &renames) {
                Some((combined, imgb)) => (combined, src_skl, target_phb, imgb, source_trbs),
                None if identity => {
                    println!(
                        "Note: identity-map combine omits the RID; using the source model directly"
                    );
                    (
                        loaded[0].0.clone(),
                        src_skl,
                        target_phb,
                        loaded[0].1.clone(),
                        source_trbs,
                    )
                }
                None => anyhow::bail!("combine failed (unexpected model layout)"),
            }
        }
        _ => {
            let imgb = std::fs::read(model.with_extension("imgb")).unwrap_or_default();
            (std::fs::read(model)?, None, Vec::new(), imgb, Vec::new())
        }
    };

    let mut result = tilde::apply_script_with_sources(&base_trb, bundle_data, &s, &source_trbs)
        .ok_or_else(|| anyhow!("apply_script failed: bad trb/bundle or unparseable script"))?;

    let renames = s.bone_renames();
    let rename_exact = s.bone_renames_add_mode();
    if s.has_skeleton_merge() {
        if let Some(src_skl) = &src_skl {
            result = merge_skeleton(
                &result,
                src_skl,
                &renames,
                &s.skeleton_e_ops(),
                s.has_skeleton_face_fix(),
                rename_exact,
                s.has_skeleton_s(),
            )?;
            println!("Merged skeleton ({} rename rules)", renames.len());
        } else {
            println!(
                "Note: script has a skeleton merge but no --orig source; skeleton left unmerged"
            );
        }
    }

    if let Some(src_skl) = &src_skl {
        rename_phb_bones(&mut result, src_skl, &renames, rename_exact, &target_phb)?;
    }

    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(out, &result)?;
    if !imgb.is_empty() {
        pad16(&mut imgb);
        std::fs::write(out.with_extension("imgb"), &imgb)?;
    }
    println!(
        "Applied {} op(s); wrote {} ({} KiB trb, {} KiB imgb)",
        s.ops.len(),
        out.display(),
        result.len() / 1024,
        imgb.len() / 1024
    );
    Ok(())
}

/// Install the model portion of an "FF XIII HD" pack from its edit scripts.
pub fn install_ff13hd(
    white_data: &Path,
    mods_out: &Path,
    data_dir: &Path,
    dry_run: bool,
) -> Result<()> {
    let mut scripts: Vec<PathBuf> = std::fs::read_dir(data_dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .filter(|p| {
            let n = p.file_name().and_then(|f| f.to_str()).unwrap_or("");
            !n.starts_with("Load") && !is_zone_c_twin(n) // skip Load* manifests + u/c dups
        })
        .collect();
    scripts.sort();

    let ultimate = data_dir.join("FFXIII_Ultimate_Models.bin");
    let general = data_dir.join("FFXIII_General_Models.bin");
    let mut bundles: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    let mut done = 0usize;
    let mut skipped = 0usize;
    for script in &scripts {
        let text = std::fs::read_to_string(script)?.replace("\r\n", "\n");
        let s = Script::parse(&text);
        let Some(out_raw) = s.output_paths().0 else {
            skipped += 1;
            continue;
        };
        let rel = strip_zone_prefix(out_raw).replace('\\', "/");
        // Each script names exactly one bundle inline (Ultimate or General).
        let bundle_path = if text.contains("FFXIII_General_Models") {
            &general
        } else {
            &ultimate
        };
        let model = white_data.join(&rel); // source = target's stock model
        let out = mods_out.join(&rel);
        let name = script.file_name().and_then(|f| f.to_str()).unwrap_or("?");
        if dry_run {
            println!("  would install {name} -> {rel}");
            done += 1;
            continue;
        }
        let bundle_data = match bundles.entry(bundle_path.clone()) {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => match std::fs::read(bundle_path) {
                Ok(b) => v.insert(b),
                Err(e) => {
                    println!("  WARN {name}: {e}");
                    skipped += 1;
                    continue;
                }
            },
        };
        match run_model_script_bytes(&model, script, bundle_data, &out, Some(white_data)) {
            Ok(()) => done += 1,
            Err(e) => {
                println!("  WARN {name}: {e}");
                skipped += 1;
            }
        }
    }
    println!("Models: {done} script(s) installed, {skipped} skipped");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_zone_prefix_drops_zoneu_zonec() {
        assert_eq!(
            strip_zone_prefix(r"zoneu\z010\bg\loc014\m022\bin\block022.win32.trb"),
            r"bg\loc014\m022\bin\block022.win32.trb"
        );
        assert_eq!(
            strip_zone_prefix(r"zonec\z010\bg\loc014\m041\bin\block041.win32.trb"),
            r"bg\loc014\m041\bin\block041.win32.trb"
        );
    }

    #[test]
    fn strip_zone_prefix_leaves_normal_paths() {
        assert_eq!(
            strip_zone_prefix(r"chr\pc\c001\bin\c001.win32.trb"),
            r"chr\pc\c001\bin\c001.win32.trb"
        );
        // 2nd segment not `z<digits>` -> not a zone prefix.
        assert_eq!(strip_zone_prefix(r"zoneu\misc\thing"), r"zoneu\misc\thing");
        // Non-numeric `z` segment left alone.
        assert_eq!(strip_zone_prefix(r"zoneu\zone\x"), r"zoneu\zone\x");
    }

    #[test]
    fn is_zone_c_twin_only_matches_zone_block_c() {
        assert!(is_zone_c_twin("z010block022c.txt"));
        assert!(is_zone_c_twin("z019block041c.txt"));
        // u-variant kept.
        assert!(!is_zone_c_twin("z010block022u.txt"));
        // Character scripts start with 'c' but aren't zone blocks.
        assert!(!is_zone_c_twin("c001.txt"));
        assert!(!is_zone_c_twin("c205.txt"));
        // No region suffix.
        assert!(!is_zone_c_twin("z010block022.txt"));
    }

    #[test]
    fn outfit_swap_without_models_is_an_error() {
        let dir = std::env::temp_dir().join(format!("ff13_modelops_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("empty.txt");
        std::fs::write(&script, "chr\\pc\\c201\\bin\\c201.win32.trb\n1\n0,0\n:X\n").unwrap();
        let err = build_outfit_swap(&script, &dir).unwrap_err();
        assert!(err.to_string().contains("no source models"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
