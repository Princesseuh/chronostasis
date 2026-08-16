use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::Subcommand;

use ff13::formats::imgb;

#[derive(Subcommand)]
pub enum TexAction {
    /// List the textures in a header+imgb pair (indices match extract/replace).
    List {
        /// Header file (a `.trb` or extracted WPD member with GTEX chunks).
        header: PathBuf,
        /// The paired `.imgb` data file.
        imgb: PathBuf,
    },
    /// Extract every texture to DDS files.
    Extract {
        header: PathBuf,
        /// The paired `.imgb` data file.
        imgb: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Replace texture #index in the imgb in place from an edited DDS.
    Replace {
        header: PathBuf,
        imgb: PathBuf,
        index: usize,
        dds: PathBuf,
    },
    /// Replace a texture in an `.xgr` + `.imgb` from a DDS of any size. Backs up to `.bak`.
    XgrRepack {
        xgr: PathBuf,
        imgb: PathBuf,
        dds: PathBuf,
        /// Member to replace (default: the only texture member, if unique).
        #[arg(long)]
        member: Option<String>,
    },
}

pub fn run(action: TexAction) -> Result<()> {
    match action {
        TexAction::List {
            header,
            imgb: imgb_path,
        } => {
            let h = std::fs::read(&header)?;
            let len = std::fs::metadata(&imgb_path)?.len() as usize;
            let texs = imgb::valid_gtex(&h, len);
            println!("{} texture(s) in {}", texs.len(), header.display());
            for (i, g) in texs.iter().enumerate() {
                println!(
                    "  [{i}] {}x{} fmt={} mips={} (GTEX @0x{:X})",
                    g.width, g.height, g.format, g.mip_count, g.offset
                );
            }
        }
        TexAction::Extract {
            header,
            imgb: imgb_path,
            out,
        } => {
            let h = std::fs::read(&header)?;
            let data = std::fs::read(&imgb_path)?;
            let texs = imgb::extract(&h, &data)?;
            let out_dir =
                out.unwrap_or_else(|| header.parent().map(|p| p.to_path_buf()).unwrap_or_default());
            let stem = header.file_stem().unwrap_or_default().to_string_lossy();
            std::fs::create_dir_all(&out_dir)?;
            for (i, t) in texs.iter().enumerate() {
                let dst = out_dir.join(format!("{stem}_{i}.dds"));
                std::fs::write(&dst, &t.dds)?;
                println!("  [{i}] {}x{} -> {}", t.width, t.height, dst.display());
            }
        }
        TexAction::Replace {
            header,
            imgb: imgb_path,
            index,
            dds,
        } => {
            let h = std::fs::read(&header)?;
            let mut data = std::fs::read(&imgb_path)?;
            let gtex = imgb::valid_gtex(&h, data.len())
                .into_iter()
                .nth(index)
                .ok_or_else(|| anyhow!("no texture #{index} (file has fewer)"))?;
            let bak = imgb_path.with_extension("imgb.bak");
            if !bak.exists() {
                std::fs::write(&bak, &data)?;
            }
            let dds_bytes = std::fs::read(&dds)?;
            imgb::replace_in_place(&gtex, &mut data, &dds_bytes)?;
            std::fs::write(&imgb_path, &data)?;
            println!("Replaced texture #{index} in {}", imgb_path.display());
        }
        TexAction::XgrRepack {
            xgr,
            imgb: imgb_path,
            dds,
            member,
        } => {
            let xgr_bytes = std::fs::read(&xgr)?;
            let imgb_bytes = std::fs::read(&imgb_path)?;
            let members = imgb::xgr_texture_members(&xgr_bytes, &imgb_bytes)?;
            let target = match member {
                Some(m) => m,
                None if members.len() == 1 => members[0].clone(),
                None => anyhow::bail!(
                    "{} has {} texture members ({}); pass --member",
                    xgr.display(),
                    members.len(),
                    members.join(", ")
                ),
            };
            if !members.contains(&target) {
                anyhow::bail!(
                    "no texture member '{target}' (have: {})",
                    members.join(", ")
                );
            }
            let dds_bytes = std::fs::read(&dds)?;
            let mut map = std::collections::HashMap::new();
            map.insert(target.clone(), dds_bytes);
            let (new_xgr, new_imgb) = imgb::repack_xgr(&xgr_bytes, &imgb_bytes, &map)?;
            let xgr_bak = xgr.with_extension("xgr.bak");
            let imgb_bak = imgb_path.with_extension("imgb.bak");
            if !xgr_bak.exists() {
                std::fs::write(&xgr_bak, &xgr_bytes)?;
            }
            if !imgb_bak.exists() {
                std::fs::write(&imgb_bak, &imgb_bytes)?;
            }
            std::fs::write(&xgr, &new_xgr)?;
            std::fs::write(&imgb_path, &new_imgb)?;
            println!(
                "Replaced '{target}' in {} (imgb {} -> {} bytes)",
                xgr.display(),
                imgb_bytes.len(),
                new_imgb.len()
            );
        }
    }
    Ok(())
}
