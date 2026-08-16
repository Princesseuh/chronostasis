use std::path::{Path, PathBuf};

use ff13::Game;
use ff13::formats::{wdb, ztr};

#[derive(Default)]
pub struct TextPreview {
    loaded_for: Option<PathBuf>,
    content: String,
    kind: String,
    save_ext: &'static str,
    status: Option<String>,
}

impl TextPreview {
    pub fn invalidate(&mut self) {
        self.loaded_for = None;
    }
}

/// Above this the text widget gets sluggish, so a note and a Save button stand in.
const MAX_RENDER: usize = 200_000;

pub fn show(tp: &mut TextPreview, path: &Path, game: Game, ui: &mut egui::Ui) {
    if tp.loaded_for.as_deref() != Some(path) {
        load(tp, path, game);
        tp.loaded_for = Some(path.to_path_buf());
    }

    ui.horizontal(|ui| {
        ui.strong(file_name(path));
        ui.label(&tp.kind);
        if !tp.content.is_empty() && ui.button(format!("Save .{}…", tp.save_ext)).clicked() {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "out".into());
            if let Some(p) = rfd::FileDialog::new()
                .set_file_name(format!("{stem}.{}", tp.save_ext))
                .save_file()
            {
                tp.status = Some(match std::fs::write(&p, &tp.content) {
                    Ok(()) => format!("wrote {}", p.display()),
                    Err(e) => format!("write failed: {e}"),
                });
            }
        }
    });

    if let Some(s) = &tp.status {
        ui.colored_label(egui::Color32::from_rgb(210, 180, 90), s);
    }
    if tp.content.is_empty() {
        return;
    }
    if tp.content.len() > MAX_RENDER {
        ui.label(format!(
            "decoded {} bytes (too large to display here); use Save to write the file.",
            tp.content.len()
        ));
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("text_preview")
        .show(ui, |ui| {
            // The preview is read-only; Save is what exports the decoded text.
            ui.add(
                egui::TextEdit::multiline(&mut tp.content.as_str())
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(28),
            );
        });
}

fn load(tp: &mut TextPreview, path: &Path, game: Game) {
    tp.content.clear();
    tp.status = None;
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            tp.status = Some(format!("read: {e}"));
            return;
        }
    };
    let ext = path
        .extension()
        .and_then(|x| x.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("txt") => {
            tp.kind = "text".into();
            tp.save_ext = "txt";
            tp.content = String::from_utf8_lossy(&bytes).into_owned();
            return;
        }
        Some("xml") => {
            tp.kind = "xml".into();
            tp.save_ext = "xml";
            tp.content = String::from_utf8_lossy(&bytes).into_owned();
            return;
        }
        Some("ztr") => {
            tp.kind = "text (.ztr)".into();
            tp.save_ext = "txt";
        }
        Some("wdb") => {
            tp.kind = "database (.wdb → json)".into();
            tp.save_ext = "json";
        }
        _ => {
            tp.status = Some("no text preview for this file type".into());
            return;
        }
    }

    // A malformed file can panic deep in a parser, which would take the whole app down.
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match ext.as_deref() {
        Some("ztr") => ztr::decode(&bytes, game),
        Some("wdb") => {
            let name = path.file_name().and_then(|f| f.to_str()).map(|f| {
                f.trim_end_matches(".wdb")
                    .trim_end_matches(".win32")
                    .to_string()
            });
            wdb::decode(&bytes, name.as_deref())
        }
        _ => unreachable!(),
    }));
    match decoded {
        Ok(Ok(s)) => tp.content = s,
        Ok(Err(e)) => tp.status = Some(format!("decode: {e}")),
        Err(_) => tp.status = Some("this file couldn't be decoded (parser error)".into()),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}
