//! Bulk media extraction over an unpacked game: `.scd` audio and `.wmp` movie containers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rayon::prelude::*;

use ff13_formats::{scd, wmp};

use crate::GameInstall;
use crate::archive::{self, Variant};

#[derive(Debug, Default, Clone, Copy)]
pub struct MediaReport {
    pub extracted: usize,
    /// Unsupported codec, malformed, or a movie group with no database.
    pub skipped: usize,
}

/// Unsupported or malformed files are skipped rather than failing the pass.
pub fn extract_audio_tree(root: &Path) -> Result<MediaReport> {
    let mut scds = Vec::new();
    collect_ext(root, "scd", &mut scds)?;
    let extracted = scds
        .par_iter()
        .filter(|p| extract_one_scd(p).unwrap_or(false))
        .count();
    Ok(MediaReport {
        extracted,
        skipped: scds.len() - extracted,
    })
}

fn extract_one_scd(scd_path: &Path) -> Result<bool> {
    let bytes = std::fs::read(scd_path)?;
    match scd::extract(&bytes) {
        Ok((ext, audio)) => {
            std::fs::write(scd_path.with_extension(ext), audio)?;
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}

/// The `movie_items` databases are located automatically from the main archive.
pub fn extract_movies(install: &GameInstall, v: Variant, out_dir: &Path) -> Result<MediaReport> {
    let dbs = load_movie_dbs(install, v)?;
    let movie_dir = install.data_dir().join("movie");
    let Ok(rd) = std::fs::read_dir(&movie_dir) else {
        return Ok(MediaReport::default());
    };

    let mut report = MediaReport::default();
    for ent in rd.flatten() {
        let wmp_path = ent.path();
        if wmp_path.extension().is_none_or(|x| x != "wmp") {
            continue;
        }
        let fname = wmp_path.file_name().unwrap_or_default().to_string_lossy();
        let (group, vo) = wmp::group_and_vo(&fname);
        // Each voice variant has its own database.
        let Some(db) = dbs.get(vo.as_str()).or_else(|| dbs.get("")) else {
            report.skipped += 1;
            continue;
        };
        match wmp::unpack(db, &wmp_path, &group, &vo, &out_dir.join(&group)) {
            Ok(n) => report.extracted += n,
            Err(_) => report.skipped += 1,
        }
    }
    Ok(report)
}

/// Keyed by voice suffix.
fn load_movie_dbs(install: &GameInstall, v: Variant) -> Result<HashMap<String, Vec<u8>>> {
    let fl = archive::read_sys_filelist(install, v)?;
    let mut img = std::fs::File::open(archive::sys_white_img(install, v))?;
    let mut dbs = HashMap::new();
    for e in fl
        .entries
        .iter()
        .filter(|e| e.path.contains("movie_items") && e.path.ends_with(".wdb"))
    {
        let vo = if e.path.contains("_us") { "_us" } else { "" };
        dbs.insert(vo.to_string(), fl.extract(&mut img, e)?);
    }
    Ok(dbs)
}

/// Case-insensitive on the extension.
fn collect_ext(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) -> Result<()> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for ent in rd.flatten() {
        let p = ent.path();
        if p.is_dir() {
            collect_ext(&p, ext, out)?;
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)) {
            out.push(p);
        }
    }
    Ok(())
}
