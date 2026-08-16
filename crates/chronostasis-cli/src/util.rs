use std::path::Path;

use anyhow::{Context, Result};

pub fn prompt_yes_no(question: &str, default: bool) -> Result<bool> {
    use std::io::Write;
    print!("{question} {} ", if default { "[Y/n]" } else { "[y/N]" });
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default,
    })
}

/// Once only: a backup from an earlier run is kept as-is.
pub fn backup_once(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    let bak = path.with_file_name(name);
    if !bak.exists() {
        std::fs::copy(path, &bak)
            .with_context(|| format!("backing up {} to {}", path.display(), bak.display()))?;
    }
    Ok(())
}
