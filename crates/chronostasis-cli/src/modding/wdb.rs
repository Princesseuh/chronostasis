use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use ff13::formats::wdb;

#[derive(Subcommand)]
pub enum WdbAction {
    /// Decode a .wdb to editable .json.
    Decode {
        wdb: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Encode edited .json back into a .wdb.
    Encode {
        json: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

pub fn run(action: WdbAction) -> Result<()> {
    match action {
        WdbAction::Decode { wdb: wdb_path, out } => {
            let bytes = std::fs::read(&wdb_path)?;
            let name = wdb_path
                .file_name()
                .and_then(|f| f.to_str())
                .map(|f| f.trim_end_matches(".wdb").trim_end_matches(".win32"));
            let json = wdb::decode(&bytes, name)?;
            let dst = out.unwrap_or_else(|| wdb_path.with_extension("json"));
            std::fs::write(&dst, json)?;
            println!("Decoded -> {}", dst.display());
        }
        WdbAction::Encode { json, out } => {
            let text = std::fs::read_to_string(&json)?;
            let bytes = wdb::encode(&text)?;
            let dst = out.unwrap_or_else(|| json.with_extension("wdb"));
            std::fs::write(&dst, bytes)?;
            println!("Encoded -> {}", dst.display());
        }
    }
    Ok(())
}
