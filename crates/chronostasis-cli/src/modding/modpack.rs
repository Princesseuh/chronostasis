use std::path::Path;

use anyhow::Result;

use ff13::mods;

use crate::resolve::resolve;

pub fn info(dir: &Path) -> Result<()> {
    let mp = mods::Modpack::read(dir)?;
    let c = &mp.config;
    println!("{} v{} by {}", c.name, c.version, c.author);
    println!("  game: {}", c.game.display_name());
    println!(
        "  components: data={} en={} jp={} ext={} code={}",
        c.has_data, c.has_en, c.has_jp, c.has_external, c.has_code
    );
    if !c.summary.is_empty() {
        println!("  {}", c.summary);
    }
    Ok(())
}

pub fn install(dir: &Path, no_en: bool, no_jp: bool) -> Result<()> {
    let mp = mods::Modpack::read(dir)?;
    let gi = resolve(mp.config.game)?;
    let opts = mods::InstallOptions {
        include_en: !no_en,
        include_jp: !no_jp,
    };
    let report = mods::install(&gi.data_dir(), &mp, &gi.backup_dir(), &gi.patch_dir(), opts)?;
    mp.write_install_state(true, report.installed_en, report.installed_jp)?;
    println!(
        "Installed {}: {} files, {} container(s) repacked, code_patch={}",
        mp.config.name, report.files, report.containers_repacked, report.code_patch
    );
    for s in &report.skipped {
        println!("  skipped: {}", s.display());
    }
    Ok(())
}

pub fn uninstall(dir: &Path) -> Result<()> {
    let mp = mods::Modpack::read(dir)?;
    let gi = resolve(mp.config.game)?;
    // A variant skipped at install has no backup, so uninstalling it deletes vanilla files.
    let opts = mods::InstallOptions {
        include_en: mp.config.en_installed.unwrap_or(true),
        include_jp: mp.config.jp_installed.unwrap_or(true),
    };
    mods::uninstall(&gi.data_dir(), &mp, &gi.backup_dir(), &gi.patch_dir(), opts)?;
    mp.write_install_state(false, false, false)?;
    println!("Uninstalled {}", mp.config.name);
    Ok(())
}
