//! CLI front-end for Chronostasis, a thin shell over the `ff13` library.

mod install;
mod manage;
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
    Install(install::InstallArgs),
    /// Remove a manually registered install. Steam-detected installs are unaffected.
    Forget {
        /// The install folder that was registered.
        path: PathBuf,
    },
    /// List detected game installations.
    List,
    /// Show details for a game (paths, LAA-patch status).
    Info {
        #[arg(value_enum)]
        game: GameArg,
    },
    /// Apply (or revert) the Large Address Aware (LAA) patch.
    Patch {
        #[arg(value_enum)]
        game: GameArg,
        /// Revert the patch instead of applying it.
        #[arg(long)]
        revert: bool,
    },
    /// Print the recommended Steam launch options for running under Proton.
    LaunchOptions,
    /// Edit all proxy settings (opens chronostasis.ini in your $EDITOR).
    Configure {
        #[arg(value_enum)]
        game: GameArg,
    },
    /// Remove the proxy DLL, restoring any backed-up d3d9.dll.
    Uninstall {
        #[arg(value_enum)]
        game: GameArg,
    },
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
        Command::Install(args) => install::run(args),
        Command::Forget { path } => manage::forget(&path),
        Command::List => manage::list(),
        Command::Info { game } => manage::info(game),
        Command::Patch { game, revert } => manage::patch(game, revert),
        Command::LaunchOptions => manage::launch_options(),
        Command::Configure { game } => manage::configure(game),
        Command::Uninstall { game } => manage::uninstall(game),
        Command::Mod { action } => modding::run(action),
        Command::Repack { game, c, from } => unpack::repack(game, c, from),
        Command::Unpack(args) => unpack::run(args),
    }
}
