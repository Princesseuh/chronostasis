//! Proton launch configuration.

use crate::Game;

/// A native Windows install loads the proxy through the normal DLL search order and needs none.
pub fn launch_options_relevant() -> bool {
    !cfg!(windows)
}

#[derive(Debug, Clone)]
pub struct ProtonOptions {
    pub native_d3d9: bool,
    /// Proton's built-in LAA override, instead of the on-disk patch.
    pub force_large_address_aware: bool,
    pub extra_env: Vec<(String, String)>,
}

impl Default for ProtonOptions {
    fn default() -> Self {
        Self {
            native_d3d9: true,
            force_large_address_aware: true,
            extra_env: Vec::new(),
        }
    }
}

impl ProtonOptions {
    pub fn recommended() -> Self {
        Self::default()
    }

    /// Drops the LAA env var when the game does not need the patch, or the exe already has it on
    /// disk, so the launch string carries only what is doing work.
    pub fn recommended_for(game: Game, laa_patched: bool) -> Self {
        Self {
            force_large_address_aware: game.laa_patch_applies() && !laa_patched,
            ..Self::default()
        }
    }

    /// Everything before `%command%` is an env-var prefix applied to the game process.
    pub fn to_launch_string(&self) -> String {
        let mut parts = Vec::new();
        if self.native_d3d9 {
            parts.push("WINEDLLOVERRIDES=\"d3d9=n,b\"".to_string());
        }
        if self.force_large_address_aware {
            parts.push("PROTON_FORCE_LARGE_ADDRESS_AWARE=1".to_string());
        }
        for (k, v) in &self.extra_env {
            parts.push(format!("{k}={v}"));
        }
        parts.push("%command%".to_string());
        parts.join(" ")
    }
}

/// Goes through Steam's `steam://rungameid/` URL so the configured launch options apply. Under
/// Proton this is the only route that works: a direct-exe launch skips the DLL override.
pub fn launch_via_steam(game: Game) -> anyhow::Result<()> {
    open_url(&format!("steam://rungameid/{}", game.app_id()))
}

#[cfg(target_os = "linux")]
fn open_url(url: &str) -> anyhow::Result<()> {
    std::process::Command::new("xdg-open").arg(url).spawn()?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) -> anyhow::Result<()> {
    std::process::Command::new("open").arg(url).spawn()?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn open_url(url: &str) -> anyhow::Result<()> {
    std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommended_string() {
        assert_eq!(
            ProtonOptions::recommended().to_launch_string(),
            "WINEDLLOVERRIDES=\"d3d9=n,b\" PROTON_FORCE_LARGE_ADDRESS_AWARE=1 %command%"
        );
    }

    #[test]
    fn drops_laa_when_redundant() {
        assert!(
            ProtonOptions::recommended_for(Game::XIII, false)
                .to_launch_string()
                .contains("PROTON_FORCE_LARGE_ADDRESS_AWARE=1")
        );
        assert!(
            !ProtonOptions::recommended_for(Game::XIII, true)
                .to_launch_string()
                .contains("PROTON_FORCE_LARGE_ADDRESS_AWARE")
        );
        assert!(
            !ProtonOptions::recommended_for(Game::LR, false)
                .to_launch_string()
                .contains("PROTON_FORCE_LARGE_ADDRESS_AWARE")
        );
    }
}
