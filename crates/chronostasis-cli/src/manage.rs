use std::path::Path;

use anyhow::{Result, anyhow};

use ff13::{discovery, launch::ProtonOptions};

use crate::resolve::{GameArg, resolve};

pub fn forget(path: &Path) -> Result<()> {
    if discovery::forget_install(path)? {
        println!("Forgot {}.", path.display());
    } else {
        println!(
            "{} was not a registered install; nothing to do.",
            path.display()
        );
    }
    Ok(())
}

pub fn list() -> Result<()> {
    let installs = discovery::find_all();
    if installs.is_empty() {
        println!("No FFXIII games found in any Steam library.");
    }
    for gi in installs {
        let patched = gi
            .is_laa_patched()
            .map(|b| if b { "LAA ✓" } else { "LAA ✗" })
            .unwrap_or("LAA ?");
        println!(
            "{:<40} {}  {}",
            gi.game.display_name(),
            gi.root.display(),
            patched
        );
    }
    Ok(())
}

pub fn info(game: GameArg) -> Result<()> {
    let gi = resolve(game.into())?;
    println!("{}", gi.game.display_name());
    println!("  root:    {}", gi.root.display());
    println!("  data:    {}", gi.data_dir().display());
    println!("  exe:     {}", gi.exe_path().display());
    println!("  LAA: {}", gi.is_laa_patched()?);
    Ok(())
}

pub fn patch(game: GameArg, revert: bool) -> Result<()> {
    let gi = resolve(game.into())?;
    let exe = gi.exe_path();
    if revert {
        let changed = ff13::laa_patch::revert_laa_patch(&exe)?;
        println!(
            "{}: {}",
            gi.game.display_name(),
            if changed {
                "LAA patch reverted"
            } else {
                "was not patched"
            }
        );
    } else {
        if !gi.game.laa_patch_applies() {
            anyhow::bail!(
                "The 4GB patch does not apply to {}.",
                gi.game.display_name()
            );
        }
        let outcome = gi.apply_laa_patch()?;
        println!("{}: {:?}", gi.game.display_name(), outcome);
    }
    Ok(())
}

pub fn launch_options() -> Result<()> {
    if ff13::launch::launch_options_relevant() {
        println!("Set this in Steam → game → Properties → Launch Options:\n");
        println!("    {}", ProtonOptions::recommended().to_launch_string());
    } else {
        println!(
            "Not needed on Windows: the proxy d3d9.dll loads directly, and the 4GB patch is applied to the exe on disk."
        );
    }
    Ok(())
}

pub fn configure(game: GameArg) -> Result<()> {
    let gi = resolve(game.into())?;
    let ini = ff13::proxy::ini_path(&gi)
        .filter(|p| p.is_file())
        .ok_or_else(|| {
            anyhow!(
                "no chronostasis.ini found for {}; run `chronostasis install` first",
                gi.game.display_name()
            )
        })?;
    let editor = std::env::var_os("VISUAL")
        .or_else(|| std::env::var_os("EDITOR"))
        .unwrap_or_else(|| {
            if on_path("nano") {
                "nano".into()
            } else {
                "vi".into()
            }
        });
    std::process::Command::new(&editor).arg(&ini).status()?;
    println!("Saved settings in {}", ini.display());
    Ok(())
}

pub fn uninstall(game: GameArg) -> Result<()> {
    let gi = resolve(game.into())?;
    let restored = ff13::proxy::undeploy(&gi)?;
    println!(
        "Removed proxy{}",
        if restored {
            " and restored the previous d3d9.dll"
        } else {
            ""
        }
    );
    Ok(())
}

fn on_path(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(cmd).is_file()))
}
