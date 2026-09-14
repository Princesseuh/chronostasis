//! High-level format and modding operations for the Final Fantasy XIII trilogy.

use std::path::PathBuf;

pub use ff13_community::{self as community};
pub use ff13_formats::{self as formats, Game};
pub mod archive;
pub mod discovery;
pub mod media;
pub mod mods;
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

    pub fn data_dir(&self) -> PathBuf {
        self.root.join(self.game.data_dir())
    }

    pub fn suite_dir(&self) -> PathBuf {
        self.root.join(".chronostasis")
    }

    pub fn backup_dir(&self) -> PathBuf {
        self.suite_dir().join("backup")
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
        assert_eq!(gi.data_dir(), PathBuf::from("/games/ff13/white_data"));
    }
}
