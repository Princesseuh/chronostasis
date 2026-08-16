use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

use ff13::{Game, GameInstall, archive, archive::Variant, discovery, launch::ProtonOptions};

use crate::resolve::{GameArg, choose_install, resolve};
use crate::util::prompt_yes_no;

/// Guided setup: detect the game, install the proxy, optionally unpack, print launch options.
///
/// Pass flags to skip the prompts and run non-interactively.
#[derive(Args)]
pub struct InstallArgs {
    /// Which game (skips detection/prompt). Omit to auto-detect.
    #[arg(value_enum)]
    game: Option<GameArg>,
    /// Set up for mods: unpack the game + enable unpacked mode (skips the prompt).
    #[arg(long, conflicts_with = "no_mods")]
    mods: bool,
    /// Just the fixes, no mods or unpack (skips the prompt).
    #[arg(long)]
    no_mods: bool,
    /// Patch the exe for 4GB / Large Address Aware (skips the prompt).
    #[arg(long, conflicts_with = "no_laa")]
    laa: bool,
    /// Don't patch the exe; rely on PROTON_FORCE_LARGE_ADDRESS_AWARE (skips the prompt).
    #[arg(long)]
    no_laa: bool,
    /// Proxy d3d9.dll to install (default: bundled, or one beside this binary).
    #[arg(long)]
    dll: Option<PathBuf>,
    /// DXVK d3d9.dll for the chainload target (default: auto-detect Proton's).
    #[arg(long)]
    dxvk: Option<PathBuf>,
    /// Unpack the Japanese language build instead of English. Only affects `--mods`.
    #[arg(long)]
    jp: bool,
    /// Use, and remember, a specific install folder. Requires the game to be named.
    #[arg(long)]
    path: Option<PathBuf>,
    /// Skip the "already set up" prompt and overwrite.
    #[arg(long)]
    force: bool,
}

pub fn run(args: InstallArgs) -> Result<()> {
    let InstallArgs {
        game,
        mods,
        no_mods,
        laa,
        no_laa,
        dll,
        dxvk,
        jp,
        path,
        force,
    } = args;
    let variant = if jp { Variant::C } else { Variant::U };
    let gi = match (game, path) {
        (Some(g), Some(p)) => {
            let g: Game = g.into();
            if !discovery::is_game_root(g, &p) {
                anyhow::bail!("{} is not a {} install", p.display(), g.display_name());
            }
            let gi = GameInstall::new(g, p);
            if let Err(e) = discovery::register_install(&gi) {
                println!("(could not save this install for next time: {e})");
            } else {
                println!("Remembered this install for future commands.");
            }
            gi
        }
        (None, Some(_)) => {
            anyhow::bail!("name the game with --path, e.g. `install xiii --path <dir>`")
        }
        (Some(g), None) => resolve(g.into())?,
        (None, None) => {
            let installs = discovery::find_all();
            match installs.len() {
                0 => anyhow::bail!(
                    "No FFXIII games found. For a non-Steam copy, use `install <game> --path <dir>`."
                ),
                1 => installs.into_iter().next().unwrap(),
                _ => choose_install(installs)?,
            }
        }
    };
    println!("Found {} at {}", gi.game.display_name(), gi.root.display());

    if ff13::proxy::read_config(&gi).is_some() && !force {
        println!("Chronostasis is already set up for this game.");
        let redo = prompt_yes_no(
            "Re-run setup? This overwrites your current settings (use `configure` to change options instead).",
            false,
        )?;
        if !redo {
            println!("\nLeaving your setup as-is.");
            if ff13::launch::launch_options_relevant() {
                let laa_now = gi.is_laa_patched().unwrap_or(false);
                println!("Launch options:\n");
                println!(
                    "    {}",
                    ProtonOptions::recommended_for(gi.game, laa_now).to_launch_string()
                );
            }
            return Ok(());
        }
    }

    let dll_path = ff13::proxy::resolve_proxy_dll(dll)?;
    let already_unpacked = gi.is_unpacked_variant(variant);
    let want_mods = match (mods, no_mods) {
        (_, true) => false,
        (true, _) => true,
        // Already unpacked, so keep unpacked mode on without prompting.
        _ if already_unpacked => true,
        _ => prompt_yes_no(
            "Do you plan to install mods? This unpacks the game (tens of GB, takes a while).",
            false,
        )?,
    };
    let want_laa = if !gi.game.laa_patch_applies() {
        if laa {
            println!(
                "Note: the 4GB patch does not apply to {}; skipping it.",
                gi.game.display_name()
            );
        }
        false
    } else {
        match (laa, no_laa) {
            (true, _) => true,
            (_, true) => false,
            _ => prompt_yes_no(
                "Patch the exe for 4GB (Large Address Aware)? On Proton you can skip it: the launch options already force it via PROTON_FORCE_LARGE_ADDRESS_AWARE.",
                false,
            )?,
        }
    };

    let config = ff13::config::SuiteConfig {
        unpacked_mode: want_mods,
        ..Default::default()
    };
    let opts = ff13::proxy::DeployOptions {
        config,
        dxvk_source: dxvk,
    };
    let report = ff13::proxy::deploy(&gi, &dll_path, &opts)?;
    println!("Installed proxy -> {}", report.dll_path.display());
    if report.backed_up_existing {
        println!("  (backed up the previous d3d9.dll to d3d9.dll.bak)");
    }
    if let ff13::proxy::DebugFontStatus::Written(p) = &report.debug_font {
        println!("  debug font -> {}", p.display());
    }
    if let ff13::proxy::DxvkStatus::NotFound = report.dxvk {
        println!("  WARNING: no DXVK found; the game will crash under Proton.");
        println!("  Re-run with --dxvk <path-to-dxvk-d3d9.dll>.");
    }

    if want_laa {
        match gi.is_laa_patched() {
            Ok(true) => println!("  LAA: exe already patched for 4GB."),
            _ => match gi.apply_laa_patch() {
                Ok(outcome) => println!("  LAA: patched the exe for 4GB ({outcome:?})."),
                Err(e) => {
                    println!("  LAA: patch failed ({e}); rely on PROTON_FORCE_LARGE_ADDRESS_AWARE.")
                }
            },
        }
    }

    if want_mods && already_unpacked {
        println!("Game is already unpacked; unpacked mode enabled.");
    } else if want_mods {
        let data_dir = gi.data_dir();
        println!(
            "Unpacking the game into {} (this writes ~GBs and takes a while) …",
            data_dir.display()
        );
        let r = archive::prepare_for_modding(&gi, variant, &data_dir, false)?;
        for (zone, err) in &r.zone_failures {
            println!("  WARN {zone}: {err}");
        }
        println!(
            "Unpacked {} files. Drop mods into {}.",
            r.main_files + r.script_files + r.zone_files,
            gi.root.join("mods").display()
        );
    }

    if ff13::launch::launch_options_relevant() {
        let laa_now = gi.is_laa_patched().unwrap_or(false);
        let launch = ProtonOptions::recommended_for(gi.game, laa_now).to_launch_string();
        println!("\nAll set! In Steam, set this game's Launch Options to:\n");
        println!("    {launch}");
    } else {
        println!("\nAll set! Launch the game from Steam as usual.");
    }
    Ok(())
}
