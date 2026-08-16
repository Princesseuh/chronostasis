use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::anyhow;
use ff13::Game;
use ff13::formats::{trb::Trb, wdb, ztr};

use crate::job::JobRunner;

#[derive(Default)]
pub struct ConvertTool {
    ztr_compress: bool,
}

pub fn tools(tool: &mut ConvertTool, job: &mut JobRunner, game: Game, ui: &mut egui::Ui) {
    let busy = job.busy();
    ui.label(format!(
        "Selected game: {}. Output is written next to the input.",
        game.display_name()
    ));
    ui.add_space(6.0);

    ui.strong("Text  ·  .ztr ↔ .txt");
    ui.horizontal(|ui| {
        if button(ui, busy, "Decode .ztr → .txt…")
            && let Some(p) = pick(&["ztr"])
        {
            let out = p.with_extension("txt");
            job.spawn(ui.ctx(), "ztr decode", move || {
                let bytes = std::fs::read(&p)?;
                std::fs::write(&out, ztr::decode(&bytes, game)?)?;
                Ok(format!("→ {}", out.display()))
            });
        }
        if button(ui, busy, "Encode .txt → .ztr…")
            && let Some(p) = pick(&["txt"])
        {
            let out = p.with_extension("ztr");
            let compress = tool.ztr_compress;
            job.spawn(ui.ctx(), "ztr encode", move || {
                let txt = std::fs::read_to_string(&p)?;
                std::fs::write(&out, ztr::encode(&txt, game, compress)?)?;
                Ok(format!("→ {}", out.display()))
            });
        }
        ui.checkbox(&mut tool.ztr_compress, "compress");
    });

    ui.add_space(6.0);
    ui.strong("Database  ·  .wdb ↔ .json");
    ui.horizontal(|ui| {
        if button(ui, busy, "Decode .wdb → .json…")
            && let Some(p) = pick(&["wdb"])
        {
            let out = p.with_extension("json");
            job.spawn(ui.ctx(), "wdb decode", move || {
                let bytes = std::fs::read(&p)?;
                let name = p.file_name().and_then(|f| f.to_str()).map(|f| {
                    f.trim_end_matches(".wdb")
                        .trim_end_matches(".win32")
                        .to_string()
                });
                std::fs::write(&out, wdb::decode(&bytes, name.as_deref())?)?;
                Ok(format!("→ {}", out.display()))
            });
        }
        if button(ui, busy, "Encode .json → .wdb…")
            && let Some(p) = pick(&["json"])
        {
            let out = p.with_extension("wdb");
            job.spawn(ui.ctx(), "wdb encode", move || {
                let text = std::fs::read_to_string(&p)?;
                std::fs::write(&out, wdb::encode(&text)?)?;
                Ok(format!("→ {}", out.display()))
            });
        }
    });

    ui.add_space(6.0);
    ui.strong("Bundle  ·  .trb + .imgb ↔ DDS folder");
    ui.horizontal(|ui| {
        if button(ui, busy, "Extract .trb → DDS folder…")
            && let Some(p) = pick(&["trb"])
        {
            job.spawn(ui.ctx(), "trb extract", move || trb_extract(p));
        }
        if button(ui, busy, "Repack DDS folder → .trb…")
            && let Some(p) = pick(&["trb"])
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            job.spawn(ui.ctx(), "trb repack", move || trb_repack(p, dir));
        }
    });

    ui.add_space(10.0);
    job.show_log(ui);
}

fn button(ui: &mut egui::Ui, busy: bool, label: &str) -> bool {
    ui.add_enabled(!busy, egui::Button::new(label)).clicked()
}

fn pick(exts: &[&str]) -> Option<PathBuf> {
    rfd::FileDialog::new().add_filter("file", exts).pick_file()
}

fn trb_extract(trb_path: PathBuf) -> anyhow::Result<String> {
    let trb_bytes = std::fs::read(&trb_path)?;
    let imgb = std::fs::read(trb_path.with_extension("imgb")).unwrap_or_default();
    let parsed = Trb::parse(&trb_bytes)?;
    let stem = trb_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "trb".into());
    let out_dir = trb_path
        .parent()
        .map(|d| d.join(format!("{stem}_dds")))
        .unwrap_or_else(|| PathBuf::from(format!("{stem}_dds")));
    std::fs::create_dir_all(&out_dir)?;
    let mut n = 0;
    for (idx, dds) in parsed.extract_textures(&imgb)? {
        std::fs::write(out_dir.join(format!("tex_{idx}.dds")), &dds)?;
        n += 1;
    }
    Ok(format!("{n} DDS → {}", out_dir.display()))
}

fn trb_repack(trb_path: PathBuf, dir: PathBuf) -> anyhow::Result<String> {
    let trb_bytes = std::fs::read(&trb_path)?;
    let parsed = Trb::parse(&trb_bytes)?;
    let mut map = HashMap::new();
    for idx in parsed.texture_resources() {
        let dds = dir.join(format!("tex_{idx}.dds"));
        map.insert(
            idx,
            std::fs::read(&dds).map_err(|e| anyhow!("{}: {e}", dds.display()))?,
        );
    }
    let orig_imgb = std::fs::read(trb_path.with_extension("imgb")).unwrap_or_default();
    let (new_trb, new_imgb) = parsed.repack(&orig_imgb, &map)?;
    let out_trb = trb_path.with_extension("repacked.trb");
    let out_imgb = trb_path.with_extension("repacked.imgb");
    std::fs::write(&out_trb, new_trb)?;
    std::fs::write(&out_imgb, new_imgb)?;
    Ok(format!("→ {} + .imgb", out_trb.display()))
}
