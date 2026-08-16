use std::path::PathBuf;

use anyhow::{Result, anyhow};

use ff13::swaps;

use crate::modding::mods_dir;
use crate::resolve::{GameArg, resolve};

pub fn run(
    game: GameArg,
    target: &str,
    donor: Option<String>,
    out: Option<PathBuf>,
    dry_run: bool,
) -> Result<()> {
    let gi = resolve(game.into())?;
    let white_data = gi.data_dir();
    let mods_out = mods_dir(&gi, out);

    let Some(donor) = donor else {
        let costumes = swaps::costumes_for(target);
        if swaps::character_codes(target).is_none() && costumes.is_empty() {
            anyhow::bail!("unknown character '{target}'");
        }
        println!(
            "Costumes for {target}: {}",
            if costumes.is_empty() {
                "(none built-in)".into()
            } else {
                costumes.join(", ")
            }
        );
        println!("Or pass a raw donor model code (e.g. n910, c605) for a generic full swap.");
        return Ok(());
    };

    let mut built: Vec<(String, Vec<u8>, Vec<u8>)> = Vec::new();
    if let Some(recipe) = swaps::find(target, &donor) {
        println!(
            "Recipe: {} + {} ({} model slot(s))",
            recipe.character,
            recipe.costume,
            recipe.targets.len()
        );
        for t in recipe.targets {
            let (trb, imgb) = swaps::build_recipe_target(t, &white_data)?;
            built.push((t.out_rel.to_string(), trb, imgb));
        }
    } else if is_model_code(&donor) {
        let codes = swaps::character_codes(target).ok_or_else(|| {
            anyhow!("unknown character '{target}' (need a name for a generic swap)")
        })?;
        println!("Generic swap: {donor} -> {target} {codes:?} (rig retargeted by name)");
        for &code in codes {
            let (out_rel, trb, imgb) = swaps::build_generic_swap(&donor, code, &white_data)?;
            built.push((out_rel, trb, imgb));
        }
    } else {
        let known = swaps::costumes_for(target);
        anyhow::bail!(
            "no recipe for '{target} {donor}'. Known costumes: {}. \
             For an arbitrary model, pass a donor model code (e.g. n910).",
            if known.is_empty() {
                "(none)".into()
            } else {
                known.join(", ")
            }
        );
    }

    for (out_rel, trb, imgb) in &built {
        let out_trb = mods_out.join(out_rel.replace('\\', "/"));
        let out_imgb = out_trb.with_extension("imgb");
        println!(
            "  {} ({} KiB trb + {} KiB imgb)",
            out_rel,
            trb.len() / 1024,
            imgb.len() / 1024
        );
        if dry_run {
            continue;
        }
        if let Some(p) = out_trb.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&out_trb, trb)?;
        // A zero-byte imgb would shadow the target's own.
        if !imgb.is_empty() {
            std::fs::write(&out_imgb, imgb)?;
        }
    }
    if dry_run {
        println!(
            "(dry run; nothing written; would write under {})",
            mods_out.display()
        );
    } else {
        println!(
            "Wrote {} model(s) under {}",
            built.len(),
            mods_out.display()
        );
    }
    Ok(())
}

fn is_model_code(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2
        && matches!(b[0], b'c' | b'n' | b'C' | b'N')
        && b[1..].iter().all(u8::is_ascii_digit)
}
