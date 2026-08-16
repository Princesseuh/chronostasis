use std::path::{Path, PathBuf};

use ff13::mods::{self, InstallOptions, Modpack};
use ff13::{Game, discovery};

use crate::app::App;
use crate::player::GREEN;

#[derive(Default)]
pub struct ModManager {
    initialized: bool,
    library: Option<PathBuf>,
    packs: Vec<Modpack>,
    scanned_for: Option<Game>,
    dirty: bool,
    selected: Option<usize>,
    loaded_detail: Option<PathBuf>,
    preview: Option<egui::TextureHandle>,
    readme: Option<String>,
    run_external: bool,
    confirm_remove: Option<PathBuf>,
}

impl ModManager {
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

impl App {
    pub(crate) fn mods_page(&mut self, ui: &mut egui::Ui) {
        if !self.mods_mgr.initialized {
            self.mods_mgr.library = discovery::modpacks_library();
            self.mods_mgr.initialized = true;
            self.mods_mgr.dirty = true;
        }
        if self.mods_mgr.dirty || self.mods_mgr.scanned_for != Some(self.game) {
            self.rescan_modpacks();
        }

        self.library_row(ui);
        ui.separator();
        self.loose_file_controls(ui);
        ui.add_space(6.0);
        ui.separator();

        if self.mods_mgr.library.is_none() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label("Set a modpacks folder to browse and install packs.");
                ui.label(
                    egui::RichText::new(
                        "Point it at a library of extracted modpacks, or import a .ncmp.",
                    )
                    .weak(),
                );
            });
            return;
        }
        if self.mods_mgr.packs.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(format!(
                    "No {} modpacks in this folder yet.",
                    self.game.display_name()
                ));
                ui.label(egui::RichText::new("Import a .ncmp to add one.").weak());
            });
            return;
        }

        let mut clicked = None;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(240.0);
                egui::ScrollArea::vertical()
                    .id_salt("mp_list")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (i, mp) in self.mods_mgr.packs.iter().enumerate() {
                            let selected = self.mods_mgr.selected == Some(i);
                            let text = if mp.config.installed {
                                egui::RichText::new(mp.config.name.as_str()).color(GREEN)
                            } else {
                                egui::RichText::new(mp.config.name.as_str())
                            };
                            if ui.selectable_label(selected, text).clicked() {
                                clicked = Some(i);
                            }
                        }
                    });
            });
            ui.separator();
            // The ScrollArea copies its parent's layout, which here would be horizontal.
            ui.vertical(|ui| {
                egui::ScrollArea::vertical()
                    .id_salt("mp_detail")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.modpack_detail(ui));
            });
        });
        if let Some(i) = clicked {
            self.mods_mgr.selected = Some(i);
            self.mods_mgr.loaded_detail = None;
        }

        self.remove_confirm_dialog(ui);
    }

    fn remove_confirm_dialog(&mut self, ui: &mut egui::Ui) {
        let Some(root) = self.mods_mgr.confirm_remove.clone() else {
            return;
        };
        let (name, installed) = self
            .mods_mgr
            .packs
            .iter()
            .find(|m| m.root == root)
            .map(|m| (m.config.name.clone(), m.config.installed))
            .unwrap_or_else(|| (root.display().to_string(), false));
        let ctx = ui.ctx().clone();
        egui::Window::new("Remove modpack?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(&ctx, |ui| {
                ui.label(format!("Delete \"{name}\" from the library?"));
                ui.label(
                    egui::RichText::new("This permanently deletes the modpack folder from disk.")
                        .weak(),
                );
                if installed {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(
                            "It's still installed. Uninstall first, or you won't be able to \
                             cleanly revert its files afterward.",
                        )
                        .color(ui.visuals().warn_fg_color),
                    );
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        self.mods_mgr.confirm_remove = None;
                    }
                    let del = egui::Button::new("Delete from disk")
                        .fill(egui::Color32::from_rgb(150, 60, 60));
                    if ui.add(del).clicked() {
                        self.mods_mgr.confirm_remove = None;
                        self.remove_modpack(ui, root.clone());
                    }
                });
            });
    }

    fn library_row(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.busy();
        ui.horizontal(|ui| {
            ui.label("Modpacks folder:");
            match &self.mods_mgr.library {
                Some(p) => {
                    ui.label(egui::RichText::new(p.display().to_string()).weak());
                }
                None => {
                    ui.label(egui::RichText::new("(none set)").weak());
                }
            }
            if ui.button("Change…").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                let _ = discovery::set_modpacks_library(&dir);
                self.mods_mgr.library = Some(dir);
                self.mods_mgr.selected = None;
                self.mods_mgr.dirty = true;
            }
            let can_import = self.mods_mgr.library.is_some() && !busy;
            if ui
                .add_enabled(can_import, egui::Button::new("Import .ncmp…"))
                .on_hover_text("Extract a community .ncmp modpack into the library folder.")
                .clicked()
                && let Some(file) = rfd::FileDialog::new()
                    .add_filter("Nova modpack", &["ncmp", "zip"])
                    .pick_file()
            {
                self.import_modpack(ui, file);
            }
            if ui.button("⟳ Rescan").clicked() {
                self.mods_mgr.dirty = true;
            }
        });
    }

    fn modpack_detail(&mut self, ui: &mut egui::Ui) {
        let Some(i) = self.mods_mgr.selected else {
            ui.weak("Select a modpack from the list.");
            return;
        };
        if i >= self.mods_mgr.packs.len() {
            return;
        }
        self.ensure_detail(i, ui.ctx());

        let unpacked = self.status.unpacked;
        let busy = self.job.busy();
        // Actions needing `&mut self` are deferred below, since the pack is borrowed here.
        let mp = &self.mods_mgr.packs[i];
        enum Action {
            Install(PathBuf),
            Uninstall(PathBuf),
        }
        let mut action = None;

        ui.add_space(6.0);
        ui.horizontal_top(|ui| {
            if let Some(tex) = &self.mods_mgr.preview {
                ui.add(
                    egui::Image::new(tex)
                        .max_width(300.0)
                        .max_height(200.0)
                        .maintain_aspect_ratio(true),
                );
                ui.add_space(12.0);
            }
            ui.vertical(|ui| {
                ui.heading(&mp.config.name);
                ui.horizontal(|ui| {
                    if !mp.config.version.is_empty() {
                        ui.label(format!("v{}", mp.config.version));
                    }
                    if !mp.config.author.is_empty() {
                        ui.label(egui::RichText::new(format!("by {}", mp.config.author)).weak());
                    }
                    if mp.config.installed {
                        ui.label(egui::RichText::new("installed").color(GREEN));
                    }
                });
                ui.add_space(6.0);
                component_chips(ui, mp);
                ui.add_space(10.0);
                if (mp.config.has_en || mp.config.has_jp) && !mp.config.installed {
                    ui.horizontal(|ui| {
                        ui.label("Voice:");
                        if mp.config.has_en {
                            ui.checkbox(&mut self.include_en, "EN");
                        }
                        if mp.config.has_jp {
                            ui.checkbox(&mut self.include_jp, "JP");
                        }
                    });
                }
                if mp.config.has_external && !mp.config.installed {
                    let hover = if cfg!(windows) {
                        "Runs the pack's External installer as part of installing."
                    } else {
                        "Runs the pack's Windows installer via protontricks in the game's Proton \
                         prefix. These external programs may not run successfully on macOS/Linux."
                    };
                    ui.checkbox(&mut self.mods_mgr.run_external, "Run external setup")
                        .on_hover_text(hover);
                }
                ui.horizontal(|ui| {
                    if mp.config.installed {
                        if ui
                            .add_enabled(!busy, egui::Button::new("Uninstall"))
                            .clicked()
                        {
                            action = Some(Action::Uninstall(mp.root.clone()));
                        }
                    } else if ui
                        .add_enabled(!busy && unpacked, egui::Button::new("  Install  "))
                        .on_hover_text(if unpacked {
                            "Copy this pack's files into the game."
                        } else {
                            "Unpack the game first (above)."
                        })
                        .clicked()
                    {
                        action = Some(Action::Install(mp.root.clone()));
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Remove"))
                        .on_hover_text("Delete this modpack from the library folder on disk.")
                        .clicked()
                    {
                        self.mods_mgr.confirm_remove = Some(mp.root.clone());
                    }
                });
            });
        });

        if mp.config.has_external {
            ui.add_space(8.0);
            let warn = ui.visuals().warn_fg_color;
            egui::Frame::none()
                .fill(ui.visuals().faint_bg_color)
                .inner_margin(egui::Margin::symmetric(8.0, 6.0))
                .rounding(egui::Rounding::same(4.0))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(
                            "This modpack has an External setup step (a Windows installer). \
                             Enable \"Run external setup\" above to run it during install; on \
                             macOS/Linux these Windows programs may not run successfully. Left \
                             off, it's skipped and the mod may not install completely, check the \
                             pack's readme for any manual steps.",
                        )
                        .color(warn),
                    );
                });
        }

        if !mp.config.summary.is_empty() {
            ui.add_space(10.0);
            ui.label(&mp.config.summary);
        }

        if let Some(readme) = &self.mods_mgr.readme {
            ui.add_space(6.0);
            egui::CollapsingHeader::new("Readme")
                .default_open(false)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(readme).monospace());
                });
        }

        match action {
            Some(Action::Install(root)) => self.install_modpack(ui, root),
            Some(Action::Uninstall(root)) => self.uninstall_modpack(ui, root),
            None => {}
        }
    }

    fn ensure_detail(&mut self, i: usize, ctx: &egui::Context) {
        let Some(mp) = self.mods_mgr.packs.get(i) else {
            return;
        };
        if self.mods_mgr.loaded_detail.as_deref() == Some(mp.root.as_path()) {
            return;
        }
        let root = mp.root.clone();
        let preview = mp.preview_image();
        let readme = mp.readme_path();
        self.mods_mgr.loaded_detail = Some(root);
        self.mods_mgr.preview = preview.and_then(|p| load_preview_texture(&p, ctx));
        self.mods_mgr.readme = readme.and_then(|p| std::fs::read_to_string(p).ok());
    }

    fn rescan_modpacks(&mut self) {
        self.mods_mgr.dirty = false;
        self.mods_mgr.scanned_for = Some(self.game);
        let packs = match &self.mods_mgr.library {
            Some(lib) => mods::list_library(lib, self.game),
            None => Vec::new(),
        };
        self.mods_mgr.packs = packs;
        match self.mods_mgr.selected {
            Some(sel) if sel >= self.mods_mgr.packs.len() => self.mods_mgr.selected = None,
            None if !self.mods_mgr.packs.is_empty() => self.mods_mgr.selected = Some(0),
            _ => {}
        }
        self.mods_mgr.loaded_detail = None;
    }

    fn install_modpack(&mut self, ui: &mut egui::Ui, root: PathBuf) {
        let Some(gi) = self.install().cloned() else {
            return;
        };
        let opts = InstallOptions {
            include_en: self.include_en,
            include_jp: self.include_jp,
        };
        let run_external = self.mods_mgr.run_external;
        self.mods_mgr.dirty = true;
        self.job.spawn(ui.ctx(), "Install modpack", move || {
            let mp = Modpack::read(&root)?;
            let mut lines = Vec::new();
            if run_external && mp.config.has_external {
                // A failed external step must not block the file install.
                match mods::run_external_setup(&mp, &gi) {
                    Ok(s) => lines.push(format!("External setup: {s}")),
                    Err(e) => lines.push(format!("External setup skipped: {e}")),
                }
            }
            let report =
                mods::install(&gi.data_dir(), &mp, &gi.backup_dir(), &gi.patch_dir(), opts)?;
            mp.write_install_state(true, report.installed_en, report.installed_jp)?;
            lines.push(format!(
                "{}: {} files, {} container(s) repacked, code_patch={}",
                mp.config.name, report.files, report.containers_repacked, report.code_patch
            ));
            Ok(lines.join("\n"))
        });
    }

    fn uninstall_modpack(&mut self, ui: &mut egui::Ui, root: PathBuf) {
        let Some(gi) = self.install().cloned() else {
            return;
        };
        self.mods_mgr.dirty = true;
        self.job.spawn(ui.ctx(), "Uninstall modpack", move || {
            let mp = Modpack::read(&root)?;
            // A variant skipped at install has no backup, so uninstalling it deletes vanilla files.
            let opts = InstallOptions {
                include_en: mp.config.en_installed.unwrap_or(true),
                include_jp: mp.config.jp_installed.unwrap_or(true),
            };
            mods::uninstall(&gi.data_dir(), &mp, &gi.backup_dir(), &gi.patch_dir(), opts)?;
            mp.write_install_state(false, false, false)?;
            Ok(format!("{}: uninstalled", mp.config.name))
        });
    }

    fn import_modpack(&mut self, ui: &mut egui::Ui, ncmp: PathBuf) {
        let Some(lib) = self.mods_mgr.library.clone() else {
            return;
        };
        self.mods_mgr.dirty = true;
        self.job.spawn(ui.ctx(), "Import modpack", move || {
            let mp = mods::import_ncmp(&ncmp, &lib)?;
            Ok(format!(
                "Imported {} v{}",
                mp.config.name, mp.config.version
            ))
        });
    }

    /// Does not touch the game.
    fn remove_modpack(&mut self, ui: &mut egui::Ui, root: PathBuf) {
        self.mods_mgr.selected = None;
        self.mods_mgr.loaded_detail = None;
        self.mods_mgr.dirty = true;
        self.job.spawn(ui.ctx(), "Remove modpack", move || {
            // Only ever delete something that is actually a modpack folder.
            if !root.join("modconfig.ini").is_file() {
                return Err(anyhow::anyhow!("not a modpack folder: {}", root.display()));
            }
            std::fs::remove_dir_all(&root)?;
            Ok(format!("Removed {}", root.display()))
        });
    }
}

fn component_chips(ui: &mut egui::Ui, mp: &Modpack) {
    ui.horizontal_wrapped(|ui| {
        let mut chip = |present: bool, text: &str| {
            if present {
                ui.label(egui::RichText::new(format!(" {text} ")).small().strong());
            }
        };
        chip(mp.config.has_data, "Data");
        chip(mp.config.has_en, "EN voice");
        chip(mp.config.has_jp, "JP voice");
        chip(mp.config.has_code, "Code patch");
        chip(mp.config.has_external, "External (skipped on Linux)");
    });
}

fn load_preview_texture(path: &Path, ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let img = image::open(path).ok()?.to_rgba8();
    let img = if img.width() > 640 {
        let h = (img.height() * 640 / img.width()).max(1);
        image::imageops::resize(&img, 640, h, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let size = [img.width() as usize, img.height() as usize];
    let ci = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture("modpack_preview", ci, egui::TextureOptions::LINEAR))
}
