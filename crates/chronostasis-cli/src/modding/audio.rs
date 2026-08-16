use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use ff13::formats::scd;

use crate::util::backup_once;

#[derive(Subcommand)]
pub enum AudioAction {
    /// Extract an SCD's audio to .ogg (music) or .wav (SFX).
    Extract {
        scd: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Replace an SCD's audio from an edited .ogg/.wav (matching the codec).
    Replace {
        scd: PathBuf,
        audio: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Extract every .scd under a directory (e.g. an unpacked tree) to sidecar .ogg/.wav.
    ExtractAll {
        /// Directory scanned recursively for `.scd` files.
        dir: PathBuf,
    },
}

pub fn run(action: AudioAction) -> Result<()> {
    match action {
        AudioAction::Extract { scd: scd_path, out } => {
            let bytes = std::fs::read(&scd_path)?;
            let (ext, audio) = scd::extract(&bytes)?;
            let dst = out.unwrap_or_else(|| scd_path.with_extension(ext));
            std::fs::write(&dst, audio)?;
            println!("Extracted -> {}", dst.display());
        }
        AudioAction::Replace {
            scd: scd_path,
            audio,
            out,
        } => {
            let scd_bytes = std::fs::read(&scd_path)?;
            let audio_bytes = std::fs::read(&audio)?;
            let new_scd = scd::replace(&scd_bytes, &audio_bytes)?;
            let dst = out.unwrap_or_else(|| scd_path.clone());
            backup_once(&dst)?;
            std::fs::write(&dst, new_scd)?;
            println!("Replaced -> {}", dst.display());
        }
        AudioAction::ExtractAll { dir } => {
            let r = ff13::media::extract_audio_tree(&dir)?;
            println!(
                "Extracted {} audio file(s) ({} skipped) under {}",
                r.extracted,
                r.skipped,
                dir.display()
            );
        }
    }
    Ok(())
}
