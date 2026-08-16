use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use ff13::{archive::Variant, formats::wmp};

use crate::resolve::{GameArg, resolve};
use crate::util::backup_once;

#[derive(Subcommand)]
pub enum MovieAction {
    /// Extract the Bink movies from a .wmp (needs the movie_items .wdb).
    Extract {
        /// The movie_items[_us].win32.wdb database.
        db: PathBuf,
        /// The .wmp container (group/voice are derived from its name).
        wmp: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Repack a directory of .bik movies into a .wmp (updates the .wdb in place).
    Repack {
        db: PathBuf,
        dir: PathBuf,
        /// The original .wmp (used for group/voice + output name).
        wmp: PathBuf,
        #[arg(long)]
        out_wmp: Option<PathBuf>,
        #[arg(long)]
        out_db: Option<PathBuf>,
    },
    /// Extract every .wmp in the game's movie/ dir (auto-locates the movie_items db).
    ExtractAll {
        #[arg(value_enum)]
        game: GameArg,
        /// Use the `c` archive variant instead of `u`.
        #[arg(long)]
        c: bool,
        /// Output dir (default: <data>/movie/_extracted).
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

pub fn run(action: MovieAction) -> Result<()> {
    let name = |p: &PathBuf| {
        p.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    };
    match action {
        MovieAction::Extract {
            db,
            wmp: wmp_path,
            out,
        } => {
            let db_bytes = std::fs::read(&db)?;
            let (group, vo) = wmp::group_and_vo(&name(&wmp_path));
            let out_dir = out.unwrap_or_else(|| {
                wmp_path
                    .parent()
                    .map(|p| p.join(&group))
                    .unwrap_or_else(|| PathBuf::from(&group))
            });
            let n = wmp::unpack(&db_bytes, &wmp_path, &group, &vo, &out_dir)?;
            println!("Extracted {n} movie(s) -> {}", out_dir.display());
        }
        MovieAction::Repack {
            db,
            dir,
            wmp: wmp_path,
            out_wmp,
            out_db,
        } => {
            let mut db_bytes = std::fs::read(&db)?;
            let (group, vo) = wmp::group_and_vo(&name(&wmp_path));
            let new_wmp = wmp::repack(&mut db_bytes, &dir, &group, &vo)?;
            let wmp_dst = out_wmp.unwrap_or(wmp_path);
            let db_dst = out_db.unwrap_or(db);
            // The pair reference each other, so a mid-write failure must not desync them.
            let wmp_tmp = wmp_dst.with_extension("wmp.tmp");
            let db_tmp = db_dst.with_extension("wdb.tmp");
            std::fs::write(&wmp_tmp, new_wmp)?;
            std::fs::write(&db_tmp, db_bytes)?;
            backup_once(&wmp_dst)?;
            backup_once(&db_dst)?;
            std::fs::rename(&wmp_tmp, &wmp_dst)?;
            std::fs::rename(&db_tmp, &db_dst)?;
            println!(
                "Repacked -> {} (+ updated {})",
                wmp_dst.display(),
                db_dst.display()
            );
        }
        MovieAction::ExtractAll { game, c, out } => {
            let gi = resolve(game.into())?;
            let variant = if c { Variant::C } else { Variant::U };
            let out_dir = out.unwrap_or_else(|| gi.data_dir().join("movie").join("_extracted"));
            let r = ff13::media::extract_movies(&gi, variant, &out_dir)?;
            println!(
                "Extracted {} movie(s) ({} skipped) -> {}",
                r.extracted,
                r.skipped,
                out_dir.display()
            );
        }
    }
    Ok(())
}
