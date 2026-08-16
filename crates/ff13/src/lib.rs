//! High-level operations for modding the Steam release of Final Fantasy XIII: locating installs,
//! unpacking and repacking archives, the LAA patch, proxy setup, and mod installs.

use std::path::{Path, PathBuf};

pub use ff13_community::{self as community};
pub use ff13_formats::{self as formats, Game};
pub use ff13_laa_patch::{self as laa_patch};

pub mod archive;
pub mod config;
pub mod discovery;
pub mod launch;
pub mod media;
pub mod mods;
pub mod proxy;
pub mod swaps;

/// A located install: the game plus the directory holding its data tree.
#[derive(Debug, Clone)]
pub struct GameInstall {
    pub game: Game,
    pub root: PathBuf,
}

impl GameInstall {
    pub fn new(game: Game, root: impl Into<PathBuf>) -> Self {
        Self {
            game,
            root: root.into(),
        }
    }

    fn join_rel(&self, rel: &str) -> PathBuf {
        rel.split('/').fold(self.root.clone(), |p, seg| p.join(seg))
    }

    pub fn data_dir(&self) -> PathBuf {
        self.root.join(self.game.data_dir())
    }

    pub fn exe_path(&self) -> PathBuf {
        self.join_rel(self.game.exe_rel_path())
    }

    /// The exe's own dir; `None` for an exe sitting at the install root.
    pub fn bin_dir(&self) -> Option<PathBuf> {
        self.exe_path().parent().map(Path::to_path_buf)
    }

    pub fn suite_dir(&self) -> PathBuf {
        self.root.join(".chronostasis")
    }

    pub fn backup_dir(&self) -> PathBuf {
        self.suite_dir().join("backup")
    }

    /// Next to the DLL, where the proxy finds them at runtime.
    pub fn patch_dir(&self) -> PathBuf {
        self.bin_dir()
            .unwrap_or_else(|| self.root.clone())
            .join("ff13-patches")
    }

    pub fn is_laa_patched(&self) -> Result<bool, laa_patch::PatchError> {
        laa_patch::is_large_address_aware(&self.exe_path())
    }

    /// FFXIII additionally keeps an `untouched.exe` for the runtime self-read redirect. Under
    /// Proton, prefer [`launch`]'s env-var override instead.
    pub fn apply_laa_patch(&self) -> Result<laa_patch::PatchOutcome, laa_patch::PatchError> {
        laa_patch::apply_laa_patch(&self.exe_path(), self.game.is_steamstub_protected())
    }

    /// Checked in either language build, and read from disk, so it holds regardless of which tool
    /// did the unpack.
    pub fn is_unpacked(&self) -> bool {
        [archive::Variant::U, archive::Variant::C]
            .into_iter()
            .any(|v| self.is_unpacked_variant(v))
    }

    /// Parses the filelist, so call it on state refreshes rather than every frame.
    pub fn is_unpacked_variant(&self, v: archive::Variant) -> bool {
        let Ok(fl) = archive::read_sys_filelist(self, v) else {
            return false;
        };
        let Some(entry) = fl.entries.iter().find(|e| !e.path.is_empty()) else {
            return false;
        };
        let target = entry
            .path
            .split('/')
            .fold(self.data_dir(), |p, seg| p.join(seg));
        target.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve_under_root() {
        let gi = GameInstall::new(Game::XIII, "/games/ff13");
        assert!(
            gi.exe_path()
                .ends_with("white_data/prog/win/bin/ffxiiiimg.exe")
        );
        assert_eq!(gi.data_dir(), PathBuf::from("/games/ff13/white_data"));
        assert!(gi.bin_dir().unwrap().ends_with("white_data/prog/win/bin"));
    }
}
