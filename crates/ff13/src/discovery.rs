//! Locating Steam and game installs cross-platform.

use std::path::{Path, PathBuf};

use crate::{Game, GameInstall};

pub fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = home_dir() {
        roots.push(home.join(".local/share/Steam"));
        roots.push(home.join(".steam/steam"));
        roots.push(home.join(".steam/root"));
        roots.push(home.join("Library/Application Support/Steam"));
    }
    if cfg!(windows) {
        roots.push(PathBuf::from("C:/Program Files (x86)/Steam"));
        roots.push(PathBuf::from("C:/Program Files/Steam"));
    }
    roots.retain(|p| p.is_dir());
    roots
}

pub fn library_steamapps() -> Vec<PathBuf> {
    let mut libs = Vec::new();
    for root in steam_roots() {
        let steamapps = root.join("steamapps");
        if steamapps.is_dir() && !libs.contains(&steamapps) {
            libs.push(steamapps.clone());
        }
        for extra in parse_library_paths(&steamapps.join("libraryfolders.vdf")) {
            let extra_steamapps = extra.join("steamapps");
            if extra_steamapps.is_dir() && !libs.contains(&extra_steamapps) {
                libs.push(extra_steamapps);
            }
        }
    }
    libs
}

/// True when `root` holds the game's data tree or its exe.
pub fn is_game_root(game: Game, root: &Path) -> bool {
    root.join(game.data_dir()).is_dir() || root.join(game.exe_rel_path()).is_file()
}

/// Steam libraries plus manually-registered paths, deduplicated by canonical root so symlinked
/// Steam roots are not listed twice.
pub fn installs(game: Game) -> Vec<GameInstall> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for steamapps in library_steamapps() {
        let root = steamapps.join("common").join(game.steam_dir_name());
        if is_game_root(game, &root) {
            candidates.push(root);
        }
    }
    for gi in manual_installs() {
        if gi.game == game {
            candidates.push(gi.root);
        }
    }

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in candidates {
        let key = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
        if seen.insert(key) {
            out.push(GameInstall::new(game, root));
        }
    }
    out
}

/// Steam libraries are searched before manual paths.
pub fn find_install(game: Game) -> Option<GameInstall> {
    installs(game).into_iter().next()
}

pub fn find_all() -> Vec<GameInstall> {
    Game::ALL.iter().filter_map(|&g| find_install(g)).collect()
}

/// The manual-install registry lives under `chronostasis/` inside it.
fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(PathBuf::from);
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg));
    }
    home_dir().map(|h| h.join(".config"))
}

fn registry_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("chronostasis").join("installs.tsv"))
}

/// Stale entries whose directory no longer looks like the game are dropped on read.
pub fn manual_installs() -> Vec<GameInstall> {
    let Some(path) = registry_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let (code, root) = line.split_once('\t')?;
            let code: u8 = code.trim().parse().ok()?;
            let game = Game::ALL.into_iter().find(|g| g.code() == code)?;
            let root = PathBuf::from(root.trim());
            is_game_root(game, &root).then(|| GameInstall::new(game, root))
        })
        .collect()
}

/// Idempotent by root path. Errors if the path does not look like `gi.game`.
pub fn register_install(gi: &GameInstall) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    if !is_game_root(gi.game, &gi.root) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!(
                "{} is not a {} install",
                gi.root.display(),
                gi.game.display_name()
            ),
        ));
    }
    let path = registry_path().ok_or_else(|| Error::new(ErrorKind::NotFound, "no config dir"))?;
    let mut entries = read_registry_lines(&path);
    let line = format!("{}\t{}", gi.game.code(), gi.root.display());
    if !entries.iter().any(|e| e == &line) {
        entries.push(line);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, entries.join("\n"))
}

/// The inverse of [`register_install`]; Steam-detected installs are unaffected.
pub fn forget_install(root: &Path) -> std::io::Result<bool> {
    let Some(path) = registry_path() else {
        return Ok(false);
    };
    let want = root.display().to_string();
    let before = read_registry_lines(&path);
    let entries: Vec<String> = before
        .iter()
        .filter(|line| line.split_once('\t').map(|(_, p)| p) != Some(want.as_str()))
        .cloned()
        .collect();
    if entries.len() == before.len() {
        return Ok(false);
    }
    std::fs::write(path, entries.join("\n"))?;
    Ok(true)
}

fn modpacks_dir_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("chronostasis").join("modpacks_dir.txt"))
}

pub fn modpacks_library() -> Option<PathBuf> {
    let text = std::fs::read_to_string(modpacks_dir_file()?).ok()?;
    let dir = PathBuf::from(text.trim());
    dir.is_dir().then_some(dir)
}

pub fn set_modpacks_library(dir: &Path) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    let path =
        modpacks_dir_file().ok_or_else(|| Error::new(ErrorKind::NotFound, "no config dir"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, dir.display().to_string())
}

fn read_registry_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Prefers the newer `dxvk/i386-windows/` layout and falls back to the older 32-bit path, which
/// is under `lib` rather than `lib64`. The most recently modified match wins.
pub fn find_proton_dxvk_d3d9() -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for steamapps in library_steamapps() {
        let common = steamapps.join("common");
        let Ok(entries) = std::fs::read_dir(&common) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let name = entry.file_name();
            if !name.to_string_lossy().to_lowercase().contains("proton") {
                continue;
            }
            for rel in [
                "files/lib/wine/dxvk/i386-windows/d3d9.dll",
                "files/lib/wine/dxvk/d3d9.dll",
                "dist/lib/wine/dxvk/d3d9.dll",
            ] {
                let cand = dir.join(rel);
                if let Ok(meta) = std::fs::metadata(&cand) {
                    let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                    if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                        best = Some((mtime, cand));
                    }
                    break;
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// A lenient line scan, not a real VDF parse.
fn parse_library_paths(vdf: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(vdf) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("\"path\"")?;
            let start = rest.find('"')? + 1;
            let end = rest[start..].find('"')? + start;
            Some(PathBuf::from(rest[start..end].replace("\\\\", "/")))
        })
        .collect()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
