//! Installing community modpacks into a detected game install.

use std::path::Path;

use anyhow::{Result, anyhow};

pub use ff13_community::modpack::{
    InstallOptions, InstallReport, ModConfig, Modpack, import_ncmp, install, list_library,
    uninstall,
};

use crate::{Game, GameInstall};

const EXTERNAL_SCRIPTS: &[&str] = &[
    "Install.bat",
    "install.bat",
    "Install.exe",
    "install.exe",
    "Setup.bat",
    "setup.bat",
    "Setup.exe",
    "setup.exe",
];

/// Off Windows this goes through the game's Proton prefix via `protontricks-launch`, which may
/// not succeed. Whatever the installer writes is not backed up, so uninstall will not undo it.
pub fn run_external_setup(modpack: &Modpack, install: &GameInstall) -> Result<String> {
    let ext_dir = modpack.root.join("External");
    let script = EXTERNAL_SCRIPTS
        .iter()
        .map(|n| ext_dir.join(n))
        .find(|p| p.is_file())
        .ok_or_else(|| anyhow!("no Install/Setup script in the pack's External folder"))?;

    // The installer takes the game path through this file, not an argument.
    let mut whitepath = install.data_dir().display().to_string();
    whitepath.push(std::path::MAIN_SEPARATOR);
    let _ = std::fs::write(ext_dir.join("whitepath.txt"), whitepath);

    run_external_script(install.game, &script)
}

#[cfg(windows)]
fn run_external_script(_game: Game, script: &Path) -> Result<String> {
    std::process::Command::new(script)
        .current_dir(script.parent().unwrap_or(Path::new(".")))
        .spawn()?;
    Ok(format!("launched {}", script.display()))
}

#[cfg(not(windows))]
fn run_external_script(game: Game, script: &Path) -> Result<String> {
    let appid = game.app_id().to_string();
    match std::process::Command::new("protontricks-launch")
        .args(["--appid", &appid])
        .arg(script)
        .spawn()
    {
        Ok(_) => Ok(format!(
            "launched {} via protontricks in the Proton prefix",
            script.display()
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(anyhow!(
            "protontricks not found. Install it, or run {} yourself in the game's Proton prefix.",
            script.display()
        )),
        Err(e) => Err(anyhow!("couldn't run the External setup: {e}")),
    }
}
