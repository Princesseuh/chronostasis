//! On-disk file formats for FFXIII, XIII-2 and Lightning Returns.

use std::io;

pub mod crypto;
pub mod d3d9shader;
pub mod elb;
pub mod imgb;
pub mod model;
pub mod mot;
pub mod phb;
pub mod scd;
pub mod sedbshd;
pub mod skl;
pub mod trb;
pub mod wdb;
pub(crate) mod wdb_dicts;
pub mod white;
pub mod wmp;
pub mod wpd;
pub mod wrb;
pub mod ztr;
pub(crate) mod ztr_dicts;

pub use imgb::{Gtex, Texture};
pub use white::{FileEntry, Filelist};
pub use wpd::{Wpd, WpdEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Game {
    XIII,
    XIII2,
    LR,
}

impl Game {
    pub const ALL: [Game; 3] = [Game::XIII, Game::XIII2, Game::LR];

    /// The internal `gameCode` the format routines switch on.
    pub fn code(self) -> u8 {
        match self {
            Game::XIII => 1,
            Game::XIII2 => 2,
            Game::LR => 3,
        }
    }

    pub fn app_id(self) -> u32 {
        match self {
            Game::XIII => 292120,
            Game::XIII2 => 292140,
            Game::LR => 345350,
        }
    }

    /// Relative to the install dir.
    pub fn data_dir(self) -> &'static str {
        match self {
            Game::XIII => "white_data",
            Game::XIII2 => "alba_data",
            Game::LR => "weiss_data",
        }
    }

    /// Relative to the install dir, forward-slashed.
    pub fn exe_rel_path(self) -> &'static str {
        match self {
            Game::XIII => "white_data/prog/win/bin/ffxiiiimg.exe",
            Game::XIII2 => "alba_data/prog/win/bin/ffxiii2img.exe",
            Game::LR => "LRFF13.exe",
        }
    }

    /// The subfolder under `steamapps/common`.
    pub fn steam_dir_name(self) -> &'static str {
        match self {
            Game::XIII => "FINAL FANTASY XIII",
            Game::XIII2 => "FINAL FANTASY XIII-2",
            Game::LR => "LIGHTNING RETURNS FINAL FANTASY XIII",
        }
    }

    /// The modpack folder id, as in `Mods/<id>`.
    pub fn nova_id(self) -> &'static str {
        match self {
            Game::XIII => "XIII",
            Game::XIII2 => "XIII-2",
            Game::LR => "XIII-LR",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Game::XIII => "Final Fantasy XIII",
            Game::XIII2 => "Final Fantasy XIII-2",
            Game::LR => "Lightning Returns: Final Fantasy XIII",
        }
    }

    /// A SteamStub exe needs the LAA patch to keep a pristine `untouched.exe` for the runtime
    /// self-read redirect, and needs the DLL to run that redirect at all.
    pub fn is_steamstub_protected(self) -> bool {
        matches!(self, Game::XIII)
    }

    /// XIII and XIII-2 are the memory-starved 32-bit titles; LR does not need it.
    pub fn laa_patch_applies(self) -> bool {
        matches!(self, Game::XIII | Game::XIII2)
    }

    pub fn from_nova_id(id: &str) -> Option<Game> {
        match id {
            "XIII" => Some(Game::XIII),
            "XIII-2" => Some(Game::XIII2),
            "XIII-LR" => Some(Game::LR),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("bad magic: expected {expected}, found {found}")]
    BadMagic { expected: String, found: String },
    #[error("malformed {format}: {detail}")]
    Malformed {
        format: &'static str,
        detail: String,
    },
    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, FormatError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_metadata_roundtrips() {
        for g in Game::ALL {
            assert_eq!(Game::from_nova_id(g.nova_id()), Some(g));
            assert!(g.exe_rel_path().ends_with(".exe"));
            assert!(matches!(g.code(), 1..=3));
        }
        assert_eq!(Game::XIII.app_id(), 292120);
    }
}
