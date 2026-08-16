//! White-archive ops: locating `filelist`/`white_img` pairs and unpacking them.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

use anyhow::Result;

pub use ff13_formats::{FileEntry, Filelist};

use crate::GameInstall;

/// The game reads the build matching its language, so an unpack has to match the language being
/// played. The on-disk suffix differs by game: XIII and XIII-2 use `u`/`c`, LR `a`/`v`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    U,
    C,
}

impl Variant {
    fn ch(self, game: crate::Game) -> char {
        match (game, self) {
            (crate::Game::LR, Variant::U) => 'a',
            (crate::Game::LR, Variant::C) => 'v',
            (_, Variant::U) => 'u',
            (_, Variant::C) => 'c',
        }
    }
}

pub fn sys_filelist(install: &GameInstall, v: Variant) -> PathBuf {
    install
        .data_dir()
        .join("sys")
        .join(format!("filelist{}.win32.bin", v.ch(install.game)))
}

pub fn sys_white_img(install: &GameInstall, v: Variant) -> PathBuf {
    install
        .data_dir()
        .join("sys")
        .join(format!("white_img{}.win32.bin", v.ch(install.game)))
}

pub fn read_sys_filelist(install: &GameInstall, v: Variant) -> Result<Filelist> {
    Ok(Filelist::read(sys_filelist(install, v), install.game)?)
}

pub fn sys_filelist_scr(install: &GameInstall, v: Variant) -> PathBuf {
    install
        .data_dir()
        .join("sys")
        .join(format!("filelist_scr{}.win32.bin", v.ch(install.game)))
}

pub fn sys_white_scr(install: &GameInstall, v: Variant) -> PathBuf {
    install
        .data_dir()
        .join("sys")
        .join(format!("white_scr{}.win32.bin", v.ch(install.game)))
}

pub fn read_filelist_at(filelist: &Path, game: crate::Game) -> Result<Filelist> {
    Ok(Filelist::read(filelist, game)?)
}

#[derive(Debug, Default)]
pub struct PrepareReport {
    pub main_files: usize,
    pub script_files: usize,
    pub zone_files: usize,
    /// The remaining zones were still unpacked.
    pub zone_failures: Vec<(String, anyhow::Error)>,
}

/// Zone archives MUST be unpacked alongside the main and script ones: unpacked mode reads per-zone
/// data as loose files, so skipping them crashes on field-zone load. LR's second main archive and
/// per-DLC archives fold into `main_files`.
pub fn prepare_for_modding(
    install: &GameInstall,
    v: Variant,
    out_root: &Path,
    dry_run: bool,
) -> Result<PrepareReport> {
    prepare_inner(install, v, out_root, dry_run, None)
}

/// As [`prepare_for_modding`], but bumps `progress` per file written; get the total beforehand
/// from a `dry_run` pass.
pub fn prepare_for_modding_with_progress(
    install: &GameInstall,
    v: Variant,
    out_root: &Path,
    progress: &AtomicUsize,
) -> Result<PrepareReport> {
    prepare_inner(install, v, out_root, false, Some(progress))
}

fn prepare_inner(
    install: &GameInstall,
    v: Variant,
    out_root: &Path,
    dry_run: bool,
    progress: Option<&AtomicUsize>,
) -> Result<PrepareReport> {
    let game = install.game;

    let mut main_files = unpack_one(
        read_sys_filelist(install, v)?,
        &sys_white_img(install, v),
        out_root,
        dry_run,
        progress,
    )?;
    for (fl_path, img_path) in lr_extra_main_pairs(install, v) {
        if !fl_path.is_file() {
            continue;
        }
        main_files += unpack_one(
            Filelist::read(&fl_path, game)?,
            &img_path,
            out_root,
            dry_run,
            progress,
        )?;
    }

    let script_files = unpack_one(
        read_filelist_at(&sys_filelist_scr(install, v), game)?,
        &sys_white_scr(install, v),
        out_root,
        dry_run,
        progress,
    )?;
    let zones = unpack_zones_inner(install, v, out_root, dry_run, progress)?;
    Ok(PrepareReport {
        main_files,
        script_files,
        zone_files: zones.files,
        zone_failures: zones.failures,
    })
}

fn unpack_one(
    fl: Filelist,
    img: &Path,
    out_root: &Path,
    dry_run: bool,
    progress: Option<&AtomicUsize>,
) -> Result<usize> {
    if dry_run {
        return Ok(fl.entries.len());
    }
    match progress {
        Some(p) => Ok(fl.unpack_all_with_progress(img, out_root, p)?),
        None => Ok(fl.unpack_all(img, out_root)?),
    }
}

/// The second system archive plus every per-DLC archive under `<data>/dlc/`. Empty for XIII and
/// XIII-2, which have neither.
pub fn lr_extra_main_pairs(install: &GameInstall, v: Variant) -> Vec<(PathBuf, PathBuf)> {
    if install.game != crate::Game::LR {
        return Vec::new();
    }
    let ch = v.ch(install.game);
    let sys = install.data_dir().join("sys");
    let mut pairs = vec![(
        sys.join(format!("filelist2{ch}.win32.bin")),
        sys.join(format!("white_img2{ch}.win32.bin")),
    )];

    let suffix = format!("img_{ch}.win32.bin");
    if let Ok(dlc) = std::fs::read_dir(install.data_dir().join("dlc")) {
        for sub in dlc.flatten() {
            let dir = sub.path();
            if !dir.is_dir() {
                continue;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for ent in rd.flatten() {
                let name = ent.file_name();
                let name = name.to_string_lossy();
                if let Some(rest) = name.strip_prefix("filelist_")
                    && rest.ends_with(&suffix)
                {
                    pairs.push((ent.path(), dir.join(format!("white_{rest}"))));
                }
            }
        }
    }
    pairs
}

#[derive(Debug, Default)]
pub struct ZoneReport {
    pub files: usize,
    /// The remaining zones were still unpacked.
    pub failures: Vec<(String, anyhow::Error)>,
}

/// Each filelist is self-contained, so a failing zone does not stop the rest and is reported in
/// the result instead.
pub fn unpack_zones(
    install: &GameInstall,
    v: Variant,
    out_root: &Path,
    dry_run: bool,
) -> Result<ZoneReport> {
    unpack_zones_inner(install, v, out_root, dry_run, None)
}

fn unpack_zones_inner(
    install: &GameInstall,
    v: Variant,
    out_root: &Path,
    dry_run: bool,
    progress: Option<&AtomicUsize>,
) -> Result<ZoneReport> {
    let mut report = ZoneReport::default();
    for (name, fl, img) in zone_filelists(install, v) {
        if dry_run {
            report.files += fl.entries.len();
            continue;
        }
        let unpacked = match progress {
            Some(p) => fl.unpack_all_with_progress(&img, out_root, p),
            None => fl.unpack_all(&img, out_root),
        };
        match unpacked {
            Ok(n) => report.files += n,
            Err(e) => report.failures.push((name, e.into())),
        }
    }
    Ok(report)
}

/// Shared by unpack and its inverse, so the two always target the same set.
fn zone_filelists(install: &GameInstall, v: Variant) -> Vec<(String, Filelist, PathBuf)> {
    let dir = install.data_dir().join("zone");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    // LR splits zones by language, but XIII's are one shared language-neutral set, so a JP unpack
    // still reads the `u` files.
    let ch = if install.game == crate::Game::LR {
        v.ch(install.game)
    } else {
        'u'
    };
    let mut out = Vec::new();
    for ent in rd.flatten() {
        let fname = ent.file_name();
        let name = fname.to_string_lossy();
        let Some(stem) = name.strip_prefix("filelist_") else {
            continue;
        };
        let (zone, img_name) = if let Some(z) = stem.strip_suffix(".win32.bin2") {
            (z, format!("white_{z}_img2.win32.bin"))
        } else if let Some(z) = stem.strip_suffix(".win32.bin") {
            (z, format!("white_{z}_img.win32.bin"))
        } else {
            continue;
        };
        if !zone.ends_with(ch) {
            continue;
        }
        let img = dir.join(&img_name);
        if !img.is_file() {
            continue;
        }
        let Ok(fl) = Filelist::read(ent.path(), install.game) else {
            continue;
        };
        out.push((name.into_owned(), fl, img));
    }
    out
}

/// Targets both language builds, so it cleans up whichever one was unpacked without the caller
/// having to know. The packed archives and anything in `mods/` are left untouched.
pub fn revert_unpack(install: &GameInstall) -> Result<usize> {
    revert_unpack_inner(install, None)
}

/// As [`revert_unpack`], but drives a progress bar.
pub fn revert_unpack_with_progress(install: &GameInstall, progress: &AtomicUsize) -> Result<usize> {
    revert_unpack_inner(install, Some(progress))
}

fn revert_unpack_inner(install: &GameInstall, progress: Option<&AtomicUsize>) -> Result<usize> {
    let out_root = install.data_dir();

    // Parse every filelist before deleting anything: LR's zone filelists are themselves loose
    // files from the `2` archive, so deletion order could otherwise strand one.
    let mut filelists: Vec<Filelist> = Vec::new();
    for v in [Variant::U, Variant::C] {
        if let Ok(fl) = read_sys_filelist(install, v) {
            filelists.push(fl);
        }
        if let Ok(fl) = read_filelist_at(&sys_filelist_scr(install, v), install.game) {
            filelists.push(fl);
        }
        for (fl_path, _img) in lr_extra_main_pairs(install, v) {
            if let Ok(fl) = Filelist::read(&fl_path, install.game) {
                filelists.push(fl);
            }
        }
        for (_name, fl, _img) in zone_filelists(install, v) {
            filelists.push(fl);
        }
    }

    let mut removed = 0usize;
    let mut dirs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for fl in &filelists {
        removed += fl.remove_unpacked(&out_root, progress);
        dirs.extend(fl.unpacked_dirs(&out_root));
    }

    // Deepest first, so a parent can go empty once its children are gone.
    let mut dirs: Vec<PathBuf> = dirs.into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in &dirs {
        let _ = std::fs::remove_dir(d);
    }
    Ok(removed)
}

/// The filelist is read fully into memory first, so the outputs may be the same paths.
pub fn repack_sys(install: &GameInstall, v: Variant, from_dir: &Path) -> Result<()> {
    let fl = read_sys_filelist(install, v)?;
    Ok(fl.repack(
        from_dir,
        sys_white_img(install, v),
        sys_filelist(install, v),
    )?)
}
