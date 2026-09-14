//! CLI for Chronostasis' format and mod-authoring operations.

mod modding;
mod resolve;
mod unpack;
mod util;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::resolve::GameArg;

#[derive(Parser)]
#[command(
    name = "chronostasis",
    about = "Chronostasis: a Final Fantasy XIII modding suite",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Author, convert, and install mods (textures, text, models, modpacks, …).
    Mod {
        #[command(subcommand)]
        action: modding::ModCommand,
    },
    /// Rebuild the main white_img + filelist from the loose tree (packed mode).
    Repack {
        #[arg(value_enum)]
        game: GameArg,
        #[arg(long)]
        c: bool,
        /// Loose tree to repack from (default: the game's data dir).
        #[arg(long)]
        from: Option<PathBuf>,
    },
    Unpack(unpack::UnpackArgs),
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Mod { action } => modding::run(action),
        Command::Repack { game, c, from } => unpack::repack(game, c, from),
        Command::Unpack(args) => unpack::run(args),
    }
}
