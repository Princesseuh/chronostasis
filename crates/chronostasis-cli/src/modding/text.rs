use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use ff13::{Game, formats::ztr};

#[derive(Subcommand)]
pub enum TextAction {
    /// Decode a .ztr to an editable .txt (`key |:| text` lines).
    Decode {
        ztr: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Encode an edited .txt back into a .ztr.
    Encode {
        txt: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        /// Byte-pair compress the text (matches the game's compressed .ztr layout).
        #[arg(long)]
        compress: bool,
    },
}

pub fn run(action: TextAction) -> Result<()> {
    match action {
        TextAction::Decode { ztr: ztr_path, out } => {
            let bytes = std::fs::read(&ztr_path)?;
            let txt = ztr::decode(&bytes, Game::XIII)?;
            let dst = out.unwrap_or_else(|| ztr_path.with_extension("txt"));
            std::fs::write(&dst, txt)?;
            println!("Decoded -> {}", dst.display());
        }
        TextAction::Encode {
            txt: txt_path,
            out,
            compress,
        } => {
            let txt = std::fs::read_to_string(&txt_path)?;
            let bytes = ztr::encode(&txt, Game::XIII, compress)?;
            let dst = out.unwrap_or_else(|| txt_path.with_extension("ztr"));
            let n = bytes.len();
            std::fs::write(&dst, bytes)?;
            println!(
                "Encoded -> {} ({} bytes, {})",
                dst.display(),
                n,
                if compress {
                    "compressed"
                } else {
                    "uncompressed"
                }
            );
        }
    }
    Ok(())
}
