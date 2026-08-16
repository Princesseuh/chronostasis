mod audio;
mod convert;
mod explorer;
mod model;
mod shader_transpile;
mod text;
mod texture;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ff13::Game;

use explorer::FileKind;

use crate::app::App;

#[derive(Default)]
pub struct ModderState {
    /// One per game, so switching games does not lose your place in each tree.
    explorers: HashMap<Game, explorer::Explorer>,
    texture: texture::TextureView,
    audio: audio::AudioPlayer,
    text: text::TextPreview,
    model: model::ModelViewer,
    convert: convert::ConvertTool,
    show_tools: bool,
    show_model: bool,
    /// Spares two stat calls per frame.
    sel_is_dir: Option<(PathBuf, bool)>,
    /// Spares a metadata read per frame.
    other: Option<OtherInfo>,
}

struct OtherInfo {
    path: PathBuf,
    size: u64,
}

impl ModderState {
    pub fn refresh_fs(&mut self) {
        for e in self.explorers.values_mut() {
            e.invalidate_fs_cache();
        }
        self.sel_is_dir = None;
        self.other = None;
        // Previews cache by path, so a job that rewrote the open file invalidates them.
        self.audio.invalidate();
        self.text.invalidate();
        self.texture.invalidate();
    }
}

impl App {
    pub(crate) fn modder_tab(&mut self, ui: &mut egui::Ui) {
        let game = self.game;
        let install_root = self.install().map(|gi| gi.root.clone());
        let data_dir = self.install().map(|gi| gi.data_dir());
        let mods_dir = self
            .install()
            .and_then(|gi| gi.data_dir().parent().map(|p| p.join("mods")));

        let explorer = self.modder.explorers.entry(game).or_default();
        if let Some(root) = &install_root {
            explorer.ensure_root(root);
        }
        // Other games' trees are not showing, so their scans are wasted work.
        for (g, e) in self.modder.explorers.iter_mut() {
            if *g != game {
                e.cancel_grid();
            }
        }

        let toolbar_frame = egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin {
            left: 8.0,
            right: 8.0,
            top: 0.0,
            bottom: 6.0,
        });
        egui::TopBottomPanel::top("mod_toolbar")
            .frame(toolbar_frame)
            .show_inside(ui, |ui| {
                let explorer = self.modder.explorers.entry(game).or_default();
                explorer::toolbar(
                    explorer,
                    install_root.as_deref(),
                    data_dir.as_deref(),
                    mods_dir.as_deref(),
                    &mut self.modder.show_tools,
                    ui,
                );
                if self.modder.show_tools {
                    ui.separator();
                    convert::tools(&mut self.modder.convert, &mut self.job, game, ui);
                }
            });

        egui::SidePanel::left("mod_tree")
            .resizable(true)
            .default_width(300.0)
            .width_range(180.0..=600.0)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("tree_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let explorer = self.modder.explorers.entry(game).or_default();
                        explorer::tree(explorer, ui);
                    });
            });

        // Keeps the edge-to-edge grid off the side panel's resize separator.
        egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(10.0, 6.0)))
            .show_inside(ui, |ui| self.modder_preview(game, ui));
    }

    fn modder_preview(&mut self, game: Game, ui: &mut egui::Ui) {
        let path = self
            .modder
            .explorers
            .get(&game)
            .and_then(explorer::Explorer::selected_path);

        let is_dir = match &path {
            Some(p) => {
                if self.modder.sel_is_dir.as_ref().map(|(cp, _)| cp) != Some(p) {
                    self.modder.sel_is_dir = Some((p.clone(), p.is_dir()));
                }
                self.modder.sel_is_dir.as_ref().is_some_and(|&(_, d)| d)
            }
            None => false,
        };

        // Anything but a folder leaves the grid, so its scan is now wasted work.
        if !is_dir && let Some(e) = self.modder.explorers.get_mut(&game) {
            e.cancel_grid();
        }

        // Only `.trb` models add to the 3D scene.
        let model_adds: Vec<std::path::PathBuf> = self
            .modder
            .explorers
            .get_mut(&game)
            .map(explorer::Explorer::take_pending_adds)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| ext_lower(p).as_deref() == Some("trb"))
            .collect();

        let Some(path) = path else {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label("Pick a file or folder from the tree on the left.");
            });
            return;
        };
        if is_dir {
            let explorer = self.modder.explorers.entry(game).or_default();
            explorer::dir_grid(explorer, ui);
            return;
        }
        match explorer::file_kind(&path) {
            FileKind::Texture => {
                let is_trb = ext_lower(&path).as_deref() == Some("trb");
                if is_trb {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.modder.show_model, false, "Textures");
                        ui.selectable_value(&mut self.modder.show_model, true, "3D model");
                    });
                    ui.separator();
                }
                if is_trb && self.modder.show_model {
                    match self.render_state.clone() {
                        Some(rs) => {
                            model::show(&mut self.modder.model, &rs, &path, &model_adds, ui)
                        }
                        None => {
                            ui.label("3D viewer needs the wgpu backend (unavailable).");
                        }
                    }
                } else {
                    texture::open_path(&mut self.modder.texture, &path, ui.ctx());
                    texture::preview(&mut self.modder.texture, ui);
                }
            }
            FileKind::Audio => {
                audio::open_path(&mut self.modder.audio, &path);
                audio::preview(&mut self.modder.audio, ui);
            }
            FileKind::Text | FileKind::Data => {
                text::show(&mut self.modder.text, &path, self.game, ui)
            }
            FileKind::Other => other_info(&mut self.modder.other, &path, ui),
        }
    }
}

fn other_info(cache: &mut Option<OtherInfo>, path: &Path, ui: &mut egui::Ui) {
    if cache.as_ref().map(|o| o.path.as_path()) != Some(path) {
        *cache = Some(OtherInfo {
            path: path.to_path_buf(),
            size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        });
    }
    ui.heading(
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    ui.label(format!(
        "{} bytes",
        cache.as_ref().map(|o| o.size).unwrap_or(0)
    ));
    ui.add_space(6.0);
    ui.weak("No preview for this file type.");
}

pub(crate) fn ext_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|x| x.to_str())
        .map(|s| s.to_ascii_lowercase())
}
