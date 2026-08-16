use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::Subcommand;

use ff13::formats::trb::Trb;

use crate::util::backup_once;

#[derive(Subcommand)]
pub enum TrbAction {
    /// Extract every texture in a TRB+imgb pair to DDS (named tex_<index>.dds).
    Extract {
        trb: PathBuf,
        imgb: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Rebuild the TRB + imgb from an edited DDS directory (supports resizing).
    Repack {
        trb: PathBuf,
        /// Directory of edited tex_<index>.dds files (from `trb extract`).
        dir: PathBuf,
        #[arg(long)]
        out_trb: Option<PathBuf>,
        #[arg(long)]
        out_imgb: Option<PathBuf>,
    },
}

pub fn run(action: TrbAction) -> Result<()> {
    match action {
        TrbAction::Extract { trb, imgb, out } => {
            let trb_bytes = std::fs::read(&trb)?;
            let imgb_bytes = std::fs::read(&imgb)?;
            let parsed = Trb::parse(&trb_bytes)?;
            let out_dir =
                out.unwrap_or_else(|| trb.parent().map(|p| p.to_path_buf()).unwrap_or_default());
            std::fs::create_dir_all(&out_dir)?;
            for (idx, dds) in parsed.extract_textures(&imgb_bytes)? {
                let dst = out_dir.join(format!("tex_{idx}.dds"));
                std::fs::write(&dst, &dds)?;
                println!("  tex_{idx} -> {}", dst.display());
            }
        }
        TrbAction::Repack {
            trb,
            dir,
            out_trb,
            out_imgb,
        } => {
            let trb_bytes = std::fs::read(&trb)?;
            let parsed = Trb::parse(&trb_bytes)?;
            let mut map = std::collections::HashMap::new();
            for idx in parsed.texture_resources() {
                let dds = dir.join(format!("tex_{idx}.dds"));
                map.insert(
                    idx,
                    std::fs::read(&dds).map_err(|e| anyhow!("reading {}: {e}", dds.display()))?,
                );
            }
            let orig_imgb = std::fs::read(trb.with_extension("imgb")).unwrap_or_default();
            let (new_trb, new_imgb) = parsed.repack(&orig_imgb, &map)?;
            let trb_dst = out_trb.unwrap_or_else(|| trb.clone());
            let imgb_dst = out_imgb.unwrap_or_else(|| trb.with_extension("imgb"));
            backup_once(&trb_dst)?;
            backup_once(&imgb_dst)?;
            std::fs::write(&trb_dst, new_trb)?;
            std::fs::write(&imgb_dst, new_imgb)?;
            println!("Repacked -> {} + {}", trb_dst.display(), imgb_dst.display());
        }
    }
    Ok(())
}
