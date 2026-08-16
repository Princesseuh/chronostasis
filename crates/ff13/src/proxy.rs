//! Deploying the in-process `d3d9.dll` proxy into a game install.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use crate::config::SuiteConfig;
use crate::{Game, GameInstall};

/// The retail games ship without this, so `debug_mode` arms the menu but draws nothing until it
/// is re-injected.
const DEBUG_FONT: &[u8] = include_bytes!("../assets/DebugFontTextureDDS.bin");

#[derive(Debug)]
pub struct DeployReport {
    pub dll_path: std::path::PathBuf,
    pub backed_up_existing: bool,
    pub launch_options: String,
    pub dxvk: DxvkStatus,
    pub debug_font: DebugFontStatus,
}

#[derive(Debug)]
pub enum DxvkStatus {
    AlreadyPresent,
    Provisioned(std::path::PathBuf),
    NotFound,
}

#[derive(Debug)]
pub enum DebugFontStatus {
    /// LR has no debug menu.
    NotNeeded,
    AlreadyPresent,
    Written(std::path::PathBuf),
}

/// The proxy config plus the DXVK chainload choice, which is deploy-only and not an ini key.
#[derive(Debug, Default)]
pub struct DeployOptions {
    pub config: SuiteConfig,
    /// `None` auto-detects a Proton-shipped DXVK.
    pub dxvk_source: Option<std::path::PathBuf>,
}

pub fn ini_path(install: &GameInstall) -> Option<std::path::PathBuf> {
    install.bin_dir().map(|b| b.join("chronostasis.ini"))
}

pub fn read_config(install: &GameInstall) -> Option<SuiteConfig> {
    let text = std::fs::read_to_string(ini_path(install)?).ok()?;
    Some(SuiteConfig::parse(&text))
}

/// Leaves the DLL alone, for a settings-only save once the proxy is deployed.
pub fn write_config(install: &GameInstall, config: &SuiteConfig) -> Result<()> {
    let path =
        ini_path(install).ok_or_else(|| anyhow!("could not resolve the game's bin directory"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, config.render())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Any pre-existing `d3d9.dll` is backed up to `d3d9.dll.bak`.
pub fn deploy(install: &GameInstall, dll_src: &Path, opts: &DeployOptions) -> Result<DeployReport> {
    if !dll_src.is_file() {
        return Err(anyhow!(
            "proxy DLL not found at {}; build it with `cargo build -p ff13-hooks --release --target i686-pc-windows-gnu`",
            dll_src.display()
        ));
    }
    let bin = install
        .bin_dir()
        .ok_or_else(|| anyhow!("could not resolve the game's bin directory"))?;
    std::fs::create_dir_all(&bin)?;

    let dll_dst = bin.join("d3d9.dll");
    let backed_up_existing = dll_dst.is_file() && {
        let bak = bin.join("d3d9.dll.bak");
        if !bak.exists() {
            std::fs::rename(&dll_dst, &bak)
                .with_context(|| format!("backing up existing {}", dll_dst.display()))?;
        }
        true
    };
    std::fs::copy(dll_src, &dll_dst).with_context(|| format!("writing {}", dll_dst.display()))?;

    std::fs::write(bin.join("chronostasis.ini"), opts.config.render())?;
    std::fs::create_dir_all(bin.join("ff13-patches")).ok();

    let dxvk = provision_dxvk(&bin, opts.dxvk_source.as_deref())?;
    let debug_font = install_debug_font(install)?;

    Ok(DeployReport {
        dll_path: dll_dst,
        backed_up_existing,
        launch_options: crate::launch::ProtonOptions::recommended_for(
            install.game,
            install.is_laa_patched().unwrap_or(false),
        )
        .to_launch_string(),
        dxvk,
        debug_font,
    })
}

/// Installed on every deploy, since it is inert without the debug patches. An existing file is
/// left untouched, in case the user swapped in their own.
fn install_debug_font(install: &GameInstall) -> Result<DebugFontStatus> {
    if !matches!(install.game, Game::XIII | Game::XIII2) {
        return Ok(DebugFontStatus::NotNeeded);
    }
    let dst = install
        .data_dir()
        .join("sys")
        .join("debug")
        .join("DebugFontTextureDDS.bin");
    if dst.is_file() {
        return Ok(DebugFontStatus::AlreadyPresent);
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&dst, DEBUG_FONT).with_context(|| format!("writing {}", dst.display()))?;
    Ok(DebugFontStatus::Written(dst))
}

/// An existing `dxvk.dll` is left alone; otherwise an explicit or detected DXVK is copied in.
fn provision_dxvk(bin: &Path, explicit: Option<&Path>) -> Result<DxvkStatus> {
    let dst = bin.join("dxvk.dll");
    if dst.is_file() {
        return Ok(DxvkStatus::AlreadyPresent);
    }
    let src = match explicit {
        Some(p) => Some(p.to_path_buf()),
        None => crate::discovery::find_proton_dxvk_d3d9(),
    };
    match src {
        Some(src) => {
            std::fs::copy(&src, &dst)
                .with_context(|| format!("copying DXVK {} -> {}", src.display(), dst.display()))?;
            Ok(DxvkStatus::Provisioned(src))
        }
        None => Ok(DxvkStatus::NotFound),
    }
}

/// Tries `override_path`, then a copy beside the running binary, then the workspace's cross-built
/// copy, then the build-time bundled one.
pub fn resolve_proxy_dll(override_path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = override_path {
        return Ok(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        let beside = exe.with_file_name("d3d9.dll");
        if beside.is_file() {
            return Ok(beside);
        }
    }
    let dev = Path::new("target/i686-pc-windows-gnu/release/d3d9.dll");
    if dev.is_file() {
        return Ok(dev.to_path_buf());
    }
    #[cfg(feature = "bundled-dll")]
    {
        let tmp = std::env::temp_dir().join("chronostasis-d3d9.dll");
        std::fs::write(&tmp, BUNDLED_DLL).context("writing bundled proxy DLL")?;
        Ok(tmp)
    }
    #[cfg(not(feature = "bundled-dll"))]
    anyhow::bail!(
        "no proxy d3d9.dll found; download it from the GitHub release and put it next to the binary, or pass an explicit path"
    )
}

/// Release CI builds the DLL first and points `CHRONOSTASIS_BUNDLED_DLL` at it.
#[cfg(feature = "bundled-dll")]
const BUNDLED_DLL: &[u8] = include_bytes!(env!("CHRONOSTASIS_BUNDLED_DLL"));

pub fn undeploy(install: &GameInstall) -> Result<bool> {
    let bin = install
        .bin_dir()
        .ok_or_else(|| anyhow!("could not resolve the game's bin directory"))?;
    let dll = bin.join("d3d9.dll");
    let bak = bin.join("d3d9.dll.bak");
    if bak.is_file() {
        std::fs::rename(&bak, &dll)?;
        Ok(true)
    } else {
        if dll.is_file() {
            std::fs::remove_file(&dll)?;
        }
        Ok(false)
    }
}
