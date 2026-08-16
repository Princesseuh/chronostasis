use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::modding::mods_dir;
use crate::resolve::{GameArg, resolve};

pub fn run(mod_dir: &Path, game: GameArg, out: Option<PathBuf>) -> Result<()> {
    let gi = resolve(game.into())?;
    let mods_out = mods_dir(&gi, out);
    let src = mod_dir.join("models").join("mod");
    if !src.is_dir() {
        anyhow::bail!(
            "no models/mod under {}; run the mod's build first",
            mod_dir.display()
        );
    }
    // The zone archive-repack staging is skipped: unpacked mode reads the loose files.
    let mut n = 0usize;
    let mut stack = vec![src.clone()];
    while let Some(d) = stack.pop() {
        for ent in std::fs::read_dir(&d)?.flatten() {
            let p = ent.path();
            if p.is_dir() {
                let name = p.file_name().and_then(|f| f.to_str()).unwrap_or("");
                if name == "zoneu" || name == "zonec" {
                    continue;
                }
                stack.push(p);
                continue;
            }
            let rel = p.strip_prefix(&src).unwrap_or(&p);
            let dest = mods_out.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&p, &dest)?;
            n += 1;
        }
    }
    println!(
        "Imported {n} files from {} into {}",
        src.display(),
        mods_out.display()
    );
    Ok(())
}
