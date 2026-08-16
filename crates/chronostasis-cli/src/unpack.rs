use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;

use ff13::{archive, archive::Variant};

use crate::resolve::{GameArg, game_name_of, resolve};
use crate::util::prompt_yes_no;

/// Unpack White archives into loose files.
///
/// Extracts the main system archive by default; `--all` does the whole game.
#[derive(Args)]
pub struct UnpackArgs {
    #[arg(value_enum)]
    game: GameArg,
    /// Unpack the WHOLE game (main + script + zones) into the loose tree for unpacked mode.
    #[arg(long)]
    all: bool,
    /// Undo a whole-game unpack. Packed archives and the mods/ folder are not touched.
    #[arg(
        long,
        conflicts_with_all = ["all", "zones", "media", "only", "dry_run", "out", "c"]
    )]
    revert: bool,
    /// Skip the confirmation prompt (for --revert).
    #[arg(long, requires = "revert")]
    yes: bool,
    /// Use the `c` archive variant instead of `u`.
    #[arg(long)]
    c: bool,
    /// Output dir (default: with --all the game's data dir; else <data>/_unpacked).
    #[arg(long)]
    out: Option<PathBuf>,
    /// (--all) Report file counts without extracting.
    #[arg(long, requires = "all")]
    dry_run: bool,
    /// (--all) Only (re)unpack the per-zone archives (bg/vfx/db/sound); skip main+script.
    #[arg(long, requires = "all")]
    zones: bool,
    /// (--all) Also explode .scd audio (-> .ogg/.wav) and .wmp movies (-> .bik) as sidecars.
    #[arg(long, requires = "all", conflicts_with_all = ["dry_run", "zones"])]
    media: bool,
    /// (single) Only files whose virtual path contains this.
    #[arg(long, conflicts_with = "all")]
    only: Option<String>,
}

pub fn run(args: UnpackArgs) -> Result<()> {
    if args.revert {
        revert(args.game, args.yes)
    } else if args.all {
        unpack_all(args)
    } else {
        unpack_single(args)
    }
}

pub fn repack(game: GameArg, c: bool, from: Option<PathBuf>) -> Result<()> {
    let gi = resolve(game.into())?;
    let variant = if c { Variant::C } else { Variant::U };
    let from_dir = from.unwrap_or_else(|| gi.data_dir());
    println!(
        "Repacking from {} into the {} archive …",
        from_dir.display(),
        gi.game.display_name()
    );
    archive::repack_sys(&gi, variant, &from_dir)?;
    println!("Done: white_img + filelist rebuilt.");
    Ok(())
}

fn revert(game: GameArg, yes: bool) -> Result<()> {
    let gi = resolve(game.into())?;
    if !gi.is_unpacked() {
        println!(
            "{} is not unpacked; nothing to remove.",
            gi.game.display_name()
        );
        return Ok(());
    }
    let proceed = yes
        || prompt_yes_no(
            "Delete the unpacked loose files and return the game to packed mode? \
             (Your mods/ folder and the packed archives are not affected.)",
            false,
        )?;
    if !proceed {
        println!("Left the unpacked files in place.");
        return Ok(());
    }
    println!("Removing unpacked files …");
    let removed = archive::revert_unpack(&gi)?;
    if let Some(cfg) = ff13::proxy::read_config(&gi) {
        let cfg = ff13::config::SuiteConfig {
            unpacked_mode: false,
            ..cfg
        };
        ff13::proxy::write_config(&gi, &cfg).context(
            "could not update chronostasis.ini: unpacked_mode is still on over a \
             packed game; set it to false with `chronostasis configure`",
        )?;
    }
    println!("Done: removed {removed} unpacked files; back to packed mode.");
    Ok(())
}

fn unpack_all(args: UnpackArgs) -> Result<()> {
    let gi = resolve(args.game.into())?;
    let variant = if args.c { Variant::C } else { Variant::U };
    let out_root = args.out.unwrap_or_else(|| gi.data_dir());
    if args.zones {
        if args.dry_run {
            let r = archive::unpack_zones(&gi, variant, &out_root, true)?;
            println!(
                "would unpack {} zone files into {}",
                r.files,
                out_root.display()
            );
        } else {
            println!("Unpacking zone archives into {} …", out_root.display());
            let r = archive::unpack_zones(&gi, variant, &out_root, false)?;
            for (zone, err) in &r.failures {
                println!("  WARN {zone}: {err}");
            }
            println!("Done: {} zone files unpacked.", r.files);
        }
    } else if args.dry_run {
        let r = archive::prepare_for_modding(&gi, variant, &out_root, true)?;
        println!(
            "would unpack {} main + {} script + {} zone = {} files into {}",
            r.main_files,
            r.script_files,
            r.zone_files,
            r.main_files + r.script_files + r.zone_files,
            out_root.display()
        );
    } else {
        println!(
            "Unpacking into {} (this writes ~GBs and takes a while) …",
            out_root.display()
        );
        let r = archive::prepare_for_modding(&gi, variant, &out_root, false)?;
        for (zone, err) in &r.zone_failures {
            println!("  WARN {zone}: {err}");
        }
        println!(
            "Done: {} main + {} script + {} zone files unpacked.",
            r.main_files, r.script_files, r.zone_files
        );
        if args.media {
            println!("Extracting media (audio + movies) …");
            let audio = ff13::media::extract_audio_tree(&out_root)?;
            let movie_out = out_root.join("movie").join("_extracted");
            let movies = ff13::media::extract_movies(&gi, variant, &movie_out)?;
            println!(
                "Media: {} audio ({} skipped); {} movies ({} skipped) -> {}",
                audio.extracted,
                audio.skipped,
                movies.extracted,
                movies.skipped,
                movie_out.display()
            );
        }
        println!("\nNext: enable unpacked mode:");
        println!(
            "    chronostasis configure {0}   (set unpacked_mode = true)",
            game_name_of(args.game.into())
        );
        println!(
            "    or `chronostasis install {0} --mods` for the full guided setup.",
            game_name_of(args.game.into())
        );
    }
    Ok(())
}

fn unpack_single(args: UnpackArgs) -> Result<()> {
    let gi = resolve(args.game.into())?;
    let variant = if args.c { Variant::C } else { Variant::U };
    // LR keeps its playable characters in a second system archive.
    let sources: Vec<(PathBuf, PathBuf)> = std::iter::once((
        archive::sys_filelist(&gi, variant),
        archive::sys_white_img(&gi, variant),
    ))
    .chain(archive::lr_extra_main_pairs(&gi, variant))
    .filter(|(f, i)| f.is_file() && i.is_file())
    .collect();
    let out = args.out.unwrap_or_else(|| gi.data_dir().join("_unpacked"));
    let (mut extracted, mut matched) = (0usize, 0usize);
    for (fl_path, img) in sources {
        let mut fl = archive::read_filelist_at(&fl_path, gi.game)?;
        if let Some(filter) = &args.only {
            fl.entries.retain(|e| e.path.contains(filter.as_str()));
            if fl.entries.is_empty() {
                continue;
            }
        }
        matched += fl.entries.len();
        println!(
            "Extracting {} file(s) into {} …",
            fl.entries.len(),
            out.display()
        );
        extracted += fl.unpack_all(&img, &out)?;
    }
    if matched == 0 {
        match &args.only {
            Some(f) => println!("no files matching {f:?} in any of this game's archives"),
            None => println!("no archives found for this install"),
        }
    } else {
        println!("Extracted {extracted} files.");
    }
    Ok(())
}
