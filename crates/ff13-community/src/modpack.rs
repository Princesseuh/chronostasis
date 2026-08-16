//! Reading and installing community `.ncmp` modpacks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use ff13_formats::Game;

#[derive(Debug, Clone)]
pub struct ModConfig {
    pub name: String,
    pub version: String,
    pub author: String,
    pub summary: String,
    pub game: Game,
    /// From `[NovaChysaliaConfig]`; the misspelling is what is on disk.
    pub has_data: bool,
    pub has_en: bool,
    pub has_jp: bool,
    pub has_external: bool,
    pub has_code: bool,
    pub installed: bool,
    /// `None` when the pack predates install-state tracking.
    pub en_installed: Option<bool>,
    pub jp_installed: Option<bool>,
    /// Pack-root-relative, from `[ModPackConfig]`.
    pub preview: Option<String>,
    pub banner: Option<String>,
    pub image: Option<String>,
    pub readme: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Modpack {
    pub root: PathBuf,
    pub config: ModConfig,
}

impl Modpack {
    pub fn read(dir: impl AsRef<Path>) -> Result<Modpack> {
        let dir = dir.as_ref();
        let ini_path = dir.join("modconfig.ini");
        let ini = Ini::parse(
            &std::fs::read_to_string(&ini_path)
                .with_context(|| format!("reading {}", ini_path.display()))?,
        );

        let pack = ini.section("ModPackConfig");
        let cfg = ini.section("NovaChysaliaConfig");

        let game_entry = pack.get("GameEntry").and_then(|s| s.parse::<u8>().ok());
        let game = match game_entry {
            Some(1) => Game::XIII,
            Some(2) => Game::XIII2,
            Some(3) => Game::LR,
            _ => return Err(anyhow!("modconfig.ini: missing/invalid GameEntry")),
        };

        let config = ModConfig {
            name: pack.get("Name").cloned().unwrap_or_default(),
            version: pack.get("Version").cloned().unwrap_or_default(),
            author: pack.get("Author").cloned().unwrap_or_default(),
            summary: pack.get("Summary").cloned().unwrap_or_default(),
            game,
            has_data: cfg.get_bool("DataPatch"),
            has_en: cfg.get_bool("ENPatch"),
            has_jp: cfg.get_bool("JPPatch"),
            has_external: cfg.get_bool("ExtPatch"),
            has_code: cfg.get_bool("CodePatch"),
            installed: cfg.get_bool("Installed"),
            en_installed: cfg.get_bool_opt("ENInstalled"),
            jp_installed: cfg.get_bool_opt("JPInstalled"),
            preview: pack.get_nonempty("Preview"),
            banner: pack.get_nonempty("Banner"),
            image: pack.get_nonempty("Image"),
            readme: pack.get_nonempty("Readme"),
        };
        Ok(Modpack {
            root: dir.to_path_buf(),
            config,
        })
    }

    /// `None` unless the field is set and the file is actually present.
    fn resolve(&self, rel: &Option<String>) -> Option<PathBuf> {
        let p = self.root.join(rel.as_ref()?);
        p.is_file().then_some(p)
    }

    /// Falls back to the banner.
    pub fn preview_image(&self) -> Option<PathBuf> {
        self.resolve(&self.config.preview)
            .or_else(|| self.resolve(&self.config.banner))
            .or_else(|| self.resolve(&self.config.image))
    }

    pub fn readme_path(&self) -> Option<PathBuf> {
        self.resolve(&self.config.readme)
    }

    /// Keeps the flags Nova reads in sync, so each tool sees the other's installs.
    pub fn write_install_state(&self, installed: bool, en: bool, jp: bool) -> Result<()> {
        let path = self.root.join("modconfig.ini");
        set_ini_bools(
            &path,
            "NovaChysaliaConfig",
            &[
                ("Installed", installed),
                ("ENInstalled", en),
                ("JPInstalled", jp),
            ],
        )
        .with_context(|| format!("updating {}", path.display()))
    }
}

/// Scans the folder and one level of subfolders, so both a flat library and Nova's
/// `Mods/<GameId>/<ModName>/` layout work. Unreadable or foreign packs are skipped.
pub fn list_library(dir: &Path, game: Game) -> Vec<Modpack> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        if p.join("modconfig.ini").is_file() {
            if let Ok(mp) = Modpack::read(&p)
                && mp.config.game == game
            {
                out.push(mp);
            }
        } else if let Ok(sub) = std::fs::read_dir(&p) {
            for s in sub.flatten() {
                let sp = s.path();
                if sp.join("modconfig.ini").is_file()
                    && let Ok(mp) = Modpack::read(&sp)
                    && mp.config.game == game
                {
                    out.push(mp);
                }
            }
        }
    }
    out.sort_by_key(|mp| mp.config.name.to_lowercase());
    out
}

/// Lays the pack out as `library_dir/<nova_id>/<Name>/`, matching Nova. Errors if the archive has
/// no valid `modconfig.ini`, or the name is already taken.
pub fn import_ncmp(ncmp: &Path, library_dir: &Path) -> Result<Modpack> {
    let file = std::fs::File::open(ncmp).with_context(|| format!("opening {}", ncmp.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .with_context(|| format!("reading {} as a zip", ncmp.display()))?;

    // Extract to a scratch dir first, so a bad or foreign pack never lands in the library.
    let staging = StagingDir(library_dir.join(".ncmp-import"));
    let _ = std::fs::remove_dir_all(&staging.0);
    std::fs::create_dir_all(&staging.0)?;
    extract_zip(&mut zip, &staging.0)?;

    // modconfig.ini may sit at the archive root or inside a single wrapping folder.
    let pack_root = find_pack_root(&staging.0)
        .ok_or_else(|| anyhow!("{}: no modconfig.ini in the archive", ncmp.display()))?;
    let mp = Modpack::read(&pack_root)?;

    let target = library_dir
        .join(mp.config.game.nova_id())
        .join(&mp.config.name);
    if target.exists() {
        return Err(anyhow!(
            "a modpack named \"{}\" is already in the library",
            mp.config.name
        ));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(&pack_root, &target).is_err() {
        // `rename` fails across filesystems.
        copy_dir(&pack_root, &target)?;
    }
    Modpack::read(&target)
}

/// Deletes the wrapped import-staging dir when dropped, so no error path leaks it.
struct StagingDir(PathBuf);

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn extract_zip(zip: &mut zip::ZipArchive<std::fs::File>, dest: &Path) -> Result<()> {
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        // `enclosed_name` is what rejects absolute paths and `..` traversal.
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut f = std::fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut f)?;
        }
    }
    Ok(())
}

fn find_pack_root(dir: &Path) -> Option<PathBuf> {
    if dir.join("modconfig.ini").is_file() {
        return Some(dir.to_path_buf());
    }
    let mut subdirs = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir());
    let first = subdirs.next()?;
    (subdirs.next().is_none() && first.join("modconfig.ini").is_file()).then_some(first)
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

const CONTAINER_EXTS: &[&str] = &["bin", "xwp", "wdb", "wpd", "wpk", "xfv", "xgr", "xwb"];

#[derive(Debug, Clone, Copy)]
pub struct InstallOptions {
    pub include_en: bool,
    pub include_jp: bool,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            include_en: true,
            include_jp: true,
        }
    }
}

#[derive(Debug, Default)]
pub struct InstallReport {
    pub files: usize,
    pub containers_repacked: usize,
    pub code_patch: bool,
    /// Files with no valid destination, e.g. a member of a missing container.
    pub skipped: Vec<PathBuf>,
    /// Recorded so uninstall restores exactly the same set.
    pub installed_en: bool,
    pub installed_jp: bool,
}

enum Mapping {
    Direct,
    Member {
        container_rel: PathBuf,
        member_name: String,
    },
    Unsupported,
}

/// Backs originals up to `backup_dir` and copies `Code/*.nccp` into `patch_dir` for the in-process
/// DLL. Assumes the game is already unpacked to a loose tree.
pub fn install(
    data_dir: &Path,
    modpack: &Modpack,
    backup_dir: &Path,
    patch_dir: &Path,
    opts: InstallOptions,
) -> Result<InstallReport> {
    let mut report = InstallReport::default();
    let mut container_groups: std::collections::BTreeMap<PathBuf, Vec<(String, PathBuf)>> =
        std::collections::BTreeMap::new();

    let selected = components(modpack, opts);
    report.installed_en = selected.contains(&"EN-Data");
    report.installed_jp = selected.contains(&"JP-Data");

    for component in selected {
        let comp_dir = modpack.root.join(component);
        let mut files = Vec::new();
        collect_files(&comp_dir, &mut files)?;
        for file in files {
            let rel = file.strip_prefix(&comp_dir).unwrap().to_path_buf();
            match classify(&rel) {
                Mapping::Direct => {
                    let target = data_dir.join(&rel);
                    backup_once(backup_dir, &rel, &target)?;
                    if let Some(p) = target.parent() {
                        std::fs::create_dir_all(p)?;
                    }
                    std::fs::copy(&file, &target)
                        .with_context(|| format!("copying to {}", target.display()))?;
                    report.files += 1;
                }
                Mapping::Member {
                    container_rel,
                    member_name,
                } => container_groups
                    .entry(container_rel)
                    .or_default()
                    .push((member_name, file)),
                Mapping::Unsupported => report.skipped.push(rel),
            }
        }
    }

    for (container_rel, members) in container_groups {
        let container = data_dir.join(&container_rel);
        if !container.is_file() {
            report.skipped.push(container_rel);
            continue;
        }
        backup_once(backup_dir, &container_rel, &container)?;
        let mut wpd = ff13_formats::Wpd::read(&container)
            .with_context(|| format!("reading container {}", container.display()))?;
        for (member_name, src) in members {
            let data = std::fs::read(&src)?;
            if wpd.find(&member_name).is_some() {
                wpd.set_member(&member_name, data);
            } else {
                let (stem, ext) = split_name(&member_name);
                wpd.entries.push(ff13_formats::WpdEntry {
                    name: stem,
                    ext,
                    data,
                });
            }
        }
        wpd.save(&container)?;
        report.containers_repacked += 1;
    }

    let code_dir = modpack.root.join("Code");
    if code_dir.is_dir() {
        for entry in std::fs::read_dir(&code_dir)? {
            let p = entry?.path();
            if p.extension().and_then(|e| e.to_str()) == Some("nccp") {
                std::fs::create_dir_all(patch_dir)?;
                std::fs::copy(&p, patch_dir.join(p.file_name().unwrap()))?;
                report.code_patch = true;
            }
        }
    }

    Ok(report)
}

/// Restores whole containers from the backup rather than unmerging per-member.
pub fn uninstall(
    data_dir: &Path,
    modpack: &Modpack,
    backup_dir: &Path,
    patch_dir: &Path,
    opts: InstallOptions,
) -> Result<()> {
    let mut restored_containers = std::collections::BTreeSet::new();

    for component in components(modpack, opts) {
        let comp_dir = modpack.root.join(component);
        let mut files = Vec::new();
        collect_files(&comp_dir, &mut files)?;
        for file in files {
            let rel = file.strip_prefix(&comp_dir).unwrap().to_path_buf();
            let restore_rel = match classify(&rel) {
                Mapping::Direct => rel,
                Mapping::Member { container_rel, .. } => {
                    if !restored_containers.insert(container_rel.clone()) {
                        continue;
                    }
                    container_rel
                }
                Mapping::Unsupported => continue,
            };
            let target = data_dir.join(&restore_rel);
            let backup = backup_dir.join(&restore_rel);
            if backup.is_file() {
                if let Some(p) = target.parent() {
                    std::fs::create_dir_all(p)?;
                }
                std::fs::copy(&backup, &target)?;
            } else if target.is_file() {
                std::fs::remove_file(&target)?;
            }
        }
    }

    // Install copied these under their own names.
    let code_dir = modpack.root.join("Code");
    if code_dir.is_dir() {
        for entry in std::fs::read_dir(&code_dir)? {
            let p = entry?.path();
            if p.extension().and_then(|e| e.to_str()) == Some("nccp") {
                let installed = patch_dir.join(p.file_name().unwrap());
                if installed.is_file() {
                    std::fs::remove_file(installed)?;
                }
            }
        }
    }
    Ok(())
}

fn components(modpack: &Modpack, opts: InstallOptions) -> Vec<&'static str> {
    let mut v = vec!["Data"];
    if opts.include_en && modpack.root.join("EN-Data").is_dir() {
        v.push("EN-Data");
    }
    if opts.include_jp && modpack.root.join("JP-Data").is_dir() {
        v.push("JP-Data");
    }
    v
}

/// A container member is a segment named `_<container>.<ext>`.
fn classify(rel: &Path) -> Mapping {
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    for (i, part) in parts.iter().enumerate() {
        if let Some(name) = part.strip_prefix('_') {
            let is_container = name
                .rsplit_once('.')
                .map(|(_, ext)| CONTAINER_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
                .unwrap_or(false);
            if is_container {
                if i + 2 != parts.len() {
                    return Mapping::Unsupported;
                }
                let mut container_rel = PathBuf::new();
                for p in &parts[..i] {
                    container_rel.push(p);
                }
                container_rel.push(name);
                return Mapping::Member {
                    container_rel,
                    member_name: parts[i + 1].clone(),
                };
            }
        }
    }
    Mapping::Direct
}

fn split_name(file_name: &str) -> (String, String) {
    match file_name.rsplit_once('.') {
        Some((stem, ext)) => (stem.to_string(), ext.to_string()),
        None => (file_name.to_string(), String::new()),
    }
}

fn backup_once(backup_dir: &Path, rel: &Path, source: &Path) -> Result<()> {
    let dest = backup_dir.join(rel);
    if !dest.exists() && source.is_file() {
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::copy(source, &dest)?;
    }
    Ok(())
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_dir() {
            collect_files(&p, out)?;
        } else {
            out.push(p);
        }
    }
    Ok(())
}

/// The `modconfig.ini` dialect allows leading-space values and case-insensitive bools.
struct Ini {
    sections: BTreeMap<String, BTreeMap<String, String>>,
}

struct Section<'a>(Option<&'a BTreeMap<String, String>>);

impl Ini {
    fn parse(text: &str) -> Ini {
        let mut sections: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        let mut current = String::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                current = name.trim().to_string();
                sections.entry(current.clone()).or_default();
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                sections
                    .entry(current.clone())
                    .or_default()
                    .insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        Ini { sections }
    }

    fn section(&self, name: &str) -> Section<'_> {
        Section(self.sections.get(name))
    }
}

impl Section<'_> {
    fn get(&self, key: &str) -> Option<&String> {
        self.0.and_then(|m| m.get(key))
    }
    fn get_bool(&self, key: &str) -> bool {
        self.get_bool_opt(key).unwrap_or(false)
    }
    fn get_bool_opt(&self, key: &str) -> Option<bool> {
        self.get(key).map(|v| v.eq_ignore_ascii_case("true"))
    }
    fn get_nonempty(&self, key: &str) -> Option<String> {
        self.get(key)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

/// Preserves the rest of the file, comments included. Missing keys are appended to the section and
/// a missing section is created, in Nova's `Key= value` style.
fn set_ini_bools(path: &Path, section: &str, kvs: &[(&str, bool)]) -> std::io::Result<()> {
    // A missing file starts fresh, but any other read error must not overwrite the ini.
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut want: BTreeMap<&str, bool> = kvs.iter().copied().collect();
    let line_for = |k: &str, v: bool| format!("{k}= {v}");
    let header = format!("[{section}]");

    match lines.iter().position(|l| l.trim() == header) {
        Some(start) => {
            let end = lines[start + 1..]
                .iter()
                .position(|l| l.trim_start().starts_with('['))
                .map(|i| start + 1 + i)
                .unwrap_or(lines.len());
            for line in lines.iter_mut().take(end).skip(start + 1) {
                let Some(key) = line.split_once('=').map(|(k, _)| k.trim().to_string()) else {
                    continue;
                };
                if let Some(v) = want.remove(key.as_str()) {
                    *line = line_for(&key, v);
                }
            }
            let insert: Vec<String> = want.iter().map(|(k, v)| line_for(k, *v)).collect();
            lines.splice(end..end, insert);
        }
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header);
            lines.extend(want.iter().map(|(k, v)| line_for(k, *v)));
        }
    }

    let mut out = lines.join("\n");
    out.push('\n');
    std::fs::write(path, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modconfig() {
        let ini = Ini::parse(
            "[ModPackConfig]\nName = Cool Mod\nGameEntry = 1\n[NovaChysaliaConfig]\nDataPatch = true\nCodePatch = false\n",
        );
        assert_eq!(
            ini.section("ModPackConfig").get("Name").unwrap(),
            "Cool Mod"
        );
        assert!(ini.section("NovaChysaliaConfig").get_bool("DataPatch"));
        assert!(!ini.section("NovaChysaliaConfig").get_bool("CodePatch"));
    }

    #[test]
    fn write_install_state_roundtrips() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ff13_ini_test_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("modconfig.ini"),
            "[ModPackConfig]\nName = TestMod\nGameEntry = 1\n[NovaChysaliaConfig]\nDataPatch = true\nInstalled = false\n",
        )
        .unwrap();

        let mp = Modpack::read(&dir).unwrap();
        assert!(!mp.config.installed);
        assert_eq!(mp.config.en_installed, None, "unrecorded state stays unset");
        mp.write_install_state(true, true, false).unwrap();

        let reread = Modpack::read(&dir).unwrap();
        assert!(reread.config.installed);
        assert_eq!(reread.config.en_installed, Some(true));
        assert_eq!(reread.config.jp_installed, Some(false));
        assert!(reread.config.has_data);
        assert_eq!(reread.config.name, "TestMod");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn classify_paths() {
        assert!(matches!(
            classify(Path::new("txtres/foo.txt")),
            Mapping::Direct
        ));
        match classify(Path::new("chr/c1/_pack.bin/tex.txb")) {
            Mapping::Member {
                container_rel,
                member_name,
            } => {
                assert_eq!(container_rel, PathBuf::from("chr/c1/pack.bin"));
                assert_eq!(member_name, "tex.txb");
            }
            _ => panic!("expected member"),
        }
        assert!(matches!(
            classify(Path::new("_a.bin/_b.wdb/x")),
            Mapping::Unsupported
        ));
    }

    #[test]
    fn install_and_uninstall_roundtrip() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let tmp = std::env::temp_dir().join(format!(
            "ff13_mods_test_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let white = tmp.join("game/white_data");
        let direct = white.join("txtres/foo.txt");
        let container = white.join("chr/pc/c001/pack.bin");
        std::fs::create_dir_all(direct.parent().unwrap()).unwrap();
        std::fs::create_dir_all(container.parent().unwrap()).unwrap();
        std::fs::write(&direct, b"ORIGINAL").unwrap();
        ff13_formats::Wpd {
            entries: vec![ff13_formats::WpdEntry {
                name: "tex".into(),
                ext: "txb".into(),
                data: b"OLD".to_vec(),
            }],
        }
        .save(&container)
        .unwrap();

        let pack = tmp.join("pack");
        std::fs::create_dir_all(pack.join("Data/txtres")).unwrap();
        std::fs::create_dir_all(pack.join("Data/chr/pc/c001/_pack.bin")).unwrap();
        std::fs::write(
            pack.join("modconfig.ini"),
            "[ModPackConfig]\nName = TestMod\nGameEntry = 1\n[NovaChysaliaConfig]\nDataPatch = true\n",
        )
        .unwrap();
        std::fs::write(pack.join("Data/txtres/foo.txt"), b"MODDED").unwrap();
        std::fs::write(pack.join("Data/chr/pc/c001/_pack.bin/tex.txb"), b"NEW").unwrap();

        let mp = Modpack::read(&pack).unwrap();
        let backup = tmp.join("backup");
        let patch = tmp.join("patch");

        let report = install(&white, &mp, &backup, &patch, InstallOptions::default()).unwrap();
        assert_eq!(report.files, 1);
        assert_eq!(report.containers_repacked, 1);
        assert_eq!(std::fs::read(&direct).unwrap(), b"MODDED");
        assert_eq!(
            std::fs::read(backup.join("txtres/foo.txt")).unwrap(),
            b"ORIGINAL"
        );
        let merged = ff13_formats::Wpd::read(&container).unwrap();
        assert_eq!(merged.find("tex.txb").unwrap().data, b"NEW");
        assert!(backup.join("chr/pc/c001/pack.bin").is_file());

        uninstall(&white, &mp, &backup, &patch, InstallOptions::default()).unwrap();
        assert_eq!(std::fs::read(&direct).unwrap(), b"ORIGINAL");
        let restored = ff13_formats::Wpd::read(&container).unwrap();
        assert_eq!(restored.find("tex.txb").unwrap().data, b"OLD");

        std::fs::remove_dir_all(&tmp).ok();
    }
}
