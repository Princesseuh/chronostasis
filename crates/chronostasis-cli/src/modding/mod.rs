mod audio;
mod hd;
mod import;
mod model_swap;
mod modpack;
mod movie;
mod text;
mod texture;
mod trb;
mod wdb;

use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use ff13::GameInstall;

use crate::resolve::GameArg;

#[derive(Subcommand)]
pub enum ModCommand {
    /// Show a modpack's metadata.
    Info {
        /// Path to the extracted modpack directory (contains modconfig.ini).
        dir: PathBuf,
    },
    /// Install a modpack into the detected game.
    Install {
        dir: PathBuf,
        /// Skip the EN voice-over variant if present.
        #[arg(long)]
        no_en: bool,
        /// Skip the JP voice-over variant if present.
        #[arg(long)]
        no_jp: bool,
    },
    /// Uninstall a previously installed modpack.
    Uninstall { dir: PathBuf },
    /// Extract/replace textures (IMGB <-> DDS) for texture mod authoring.
    Texture {
        #[command(subcommand)]
        action: texture::TexAction,
    },
    /// Convert text between .ztr and editable .txt for text mod authoring.
    Text {
        #[command(subcommand)]
        action: text::TextAction,
    },
    /// Convert databases between .wdb and editable .json for data mod authoring.
    Wdb {
        #[command(subcommand)]
        action: wdb::WdbAction,
    },
    /// Extract/repack a TRB texture bundle (supports resizing; rebuilds the imgb).
    Trb {
        #[command(subcommand)]
        action: trb::TrbAction,
    },
    /// Extract/replace audio (SCD <-> ogg for music, wav for SFX).
    Audio {
        #[command(subcommand)]
        action: audio::AudioAction,
    },
    /// Extract/repack movies (WMP <-> Bink), using the movie_items database.
    Movie {
        #[command(subcommand)]
        action: movie::MovieAction,
    },
    /// Capture a built mod's `models/mod/` tree into the LayeredFS `mods/` folder.
    Import {
        /// The mod directory (containing a built `models/mod/` tree).
        mod_dir: PathBuf,
        #[arg(value_enum)]
        game: GameArg,
        /// Write overrides elsewhere instead of `<install>/mods`.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Convert an HD texture pack's obfuscated `.bin` files into LayeredFS overrides.
    HdTextures {
        /// The pack's `textures/` directory (holds the `.bin` files, in subdirs).
        pack: PathBuf,
        #[arg(value_enum)]
        game: GameArg,
        /// Write overrides elsewhere instead of `<install>/mods`.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Report what would be swapped without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Build an outfit-swap model from an HD-Models `.txt` swap script into `<install>/mods`.
    HdSwap {
        /// The HD-Models swap script (`.txt`).
        script: PathBuf,
        #[arg(value_enum)]
        game: GameArg,
        /// Write the built model elsewhere instead of `<install>/mods`.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Report what would be built without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Swap a character's model using only game-shipped assets.
    ///
    /// A named costume uses a built-in recipe; a raw model code does a generic replacement,
    /// retargeting by bone name. With no donor, lists the character's known costumes.
    ModelSwap {
        #[arg(value_enum)]
        game: GameArg,
        /// Target character name (`lightning`, `sazh`, `hope`, `vanille`, `snow`, `fang`).
        target: String,
        /// Costume name (e.g. `lebreau`) or a raw donor model code (e.g. `n910`, `c605`).
        donor: Option<String>,
        /// Write the built model(s) elsewhere instead of `<install>/mods`.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Report what would be built without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Run a full HD-Models edit script against a `.trb`.
    ///
    /// Combine scripts build their base from `--orig` first. Geometry is byte-exact, but the
    /// skeleton keeps the base rig: `:O` and 3-arg `:E` refinement are not applied.
    HdSubdivide {
        /// Target game model `.trb` (single-model scripts). Ignored for combine scripts (use `--orig`).
        model: PathBuf,
        /// The HD-Models edit script (`.txt`).
        script: PathBuf,
        /// The model bundle `.bin` (overrides the path in each op).
        #[arg(long)]
        bundle: PathBuf,
        /// Output `.trb` path.
        #[arg(long, short)]
        out: PathBuf,
        /// Source-models root dir (e.g. `…/models/orig`); required for combine scripts.
        #[arg(long)]
        orig: Option<PathBuf>,
    },
    /// Install a community mod pack into `<install>/mods`, auto-detecting its layout.
    ///
    /// Needs `unpacked_mode` and `texture_mods` set in `chronostasis.ini`.
    HdInstall {
        /// The extracted mod-pack folder.
        path: PathBuf,
        /// Which game (defaults to FFXIII).
        #[arg(value_enum, default_value = "xiii")]
        game: GameArg,
        /// Report what would be installed without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
}

pub fn run(action: ModCommand) -> Result<()> {
    match action {
        ModCommand::Info { dir } => modpack::info(&dir),
        ModCommand::Install { dir, no_en, no_jp } => modpack::install(&dir, no_en, no_jp),
        ModCommand::Uninstall { dir } => modpack::uninstall(&dir),
        ModCommand::Texture { action } => texture::run(action),
        ModCommand::Text { action } => text::run(action),
        ModCommand::Wdb { action } => wdb::run(action),
        ModCommand::Trb { action } => trb::run(action),
        ModCommand::Audio { action } => audio::run(action),
        ModCommand::Movie { action } => movie::run(action),
        ModCommand::Import { mod_dir, game, out } => import::run(&mod_dir, game, out),
        ModCommand::HdTextures {
            pack,
            game,
            out,
            dry_run,
        } => hd::textures(&pack, game, out, dry_run),
        ModCommand::HdSwap {
            script,
            game,
            out,
            dry_run,
        } => hd::swap(&script, game, out, dry_run),
        ModCommand::ModelSwap {
            game,
            target,
            donor,
            out,
            dry_run,
        } => model_swap::run(game, &target, donor, out, dry_run),
        ModCommand::HdSubdivide {
            model,
            script,
            bundle,
            out,
            orig,
        } => hd::subdivide(&model, &script, &bundle, &out, orig),
        ModCommand::HdInstall {
            path,
            game,
            dry_run,
        } => hd::install(&path, game, dry_run),
    }
}

fn mods_dir(gi: &GameInstall, out: Option<PathBuf>) -> PathBuf {
    out.unwrap_or_else(|| {
        gi.data_dir()
            .parent()
            .map(|p| p.join("mods"))
            .unwrap_or_else(|| PathBuf::from("mods"))
    })
}
