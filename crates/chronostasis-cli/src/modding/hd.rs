use std::path::{Path, PathBuf};

use anyhow::Result;

use ff13::{community::modelops, formats::trb::Trb};

use crate::modding::mods_dir;
use crate::resolve::{GameArg, resolve};

pub fn textures(pack: &Path, game: GameArg, out: Option<PathBuf>, dry_run: bool) -> Result<()> {
    let gi = resolve(game.into())?;
    let white_data = gi.data_dir();
    let mods_out = mods_dir(&gi, out);
    modelops::install_hd_textures(&white_data, &mods_out, pack, dry_run)
}

pub fn swap(script: &Path, game: GameArg, out: Option<PathBuf>, dry_run: bool) -> Result<()> {
    let gi = resolve(game.into())?;
    let white_data = gi.data_dir();
    let mods_out = mods_dir(&gi, out);

    let built = modelops::build_outfit_swap(script, &white_data)?;
    let out_trb = mods_out.join(built.out_rel.replace('\\', "/"));
    let out_imgb = out_trb.with_extension("imgb");
    println!(
        "Built {} ({} resources, {} KiB trb + {} KiB imgb) from {} source model(s)",
        built.out_rel,
        Trb::parse(&built.trb)?.resource_count(),
        built.trb.len() / 1024,
        built.imgb.len() / 1024,
        built.sources
    );
    if dry_run {
        println!(
            "  (dry run; nothing written; would write to {})",
            mods_out.display()
        );
    } else {
        if let Some(p) = out_trb.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&out_trb, &built.trb)?;
        std::fs::write(&out_imgb, &built.imgb)?;
        println!("  wrote {} + .imgb", out_trb.display());
    }
    Ok(())
}

pub fn subdivide(
    model: &Path,
    script: &Path,
    bundle: &Path,
    out: &Path,
    orig: Option<PathBuf>,
) -> Result<()> {
    modelops::run_model_script(model, script, bundle, out, orig.as_deref())
}

pub fn install(path: &Path, game: GameArg, dry_run: bool) -> Result<()> {
    let gi = resolve(game.into())?;
    let white_data = gi.data_dir();
    let mods_out = mods_dir(&gi, None);

    if path.join("hd_textures").join("hash_database.txt").is_file() {
        println!("Detected: HD Fonts and GUI (runtime hash-swap pack)");
        let hd = path.join("hd_textures");
        let r = ff13::community::hdgui::emit_layeredfs_hashed(
            &hd,
            &white_data,
            &white_data,
            &mods_out,
            dry_run,
        )?;
        let verb = if dry_run { "would write" } else { "wrote" };
        println!(
            "{verb} {} override(s) to {} ({} unmatched, {} missing .dds)",
            r.written.len(),
            mods_out.display(),
            r.unmatched.len(),
            r.missing_dds.len()
        );
    } else if path.join("Data").is_dir() && path.join("textures").is_dir() {
        println!("Detected: FF XIII HD (Ultimate models + HD textures pack)");
        modelops::install_ff13hd(
            &white_data,
            &mods_out,
            &path.join("Data"),
            &path.join("textures"),
            dry_run,
        )?;
    } else {
        anyhow::bail!(
            "could not identify the mod at {}: expected HD Fonts ('hd_textures/hash_database.txt') \
             or FF XIII HD ('Data/' + 'textures/')",
            path.display()
        );
    }
    if dry_run {
        println!("(dry run; nothing written)");
    }
    Ok(())
}
