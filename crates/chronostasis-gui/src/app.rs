use std::collections::HashMap;

use ff13::{Game, GameInstall, discovery};

use crate::job::JobRunner;

pub struct App {
    pub game: Game,
    pub installs: HashMap<Game, Vec<GameInstall>>,
    pub install_idx: HashMap<Game, usize>,
    pub add_error: Option<String>,
    pub job: JobRunner,
    pub modder: crate::modder::ModderState,
    pub render_state: Option<eframe::egui_wgpu::RenderState>,
}

impl Default for App {
    fn default() -> Self {
        let mut app = Self {
            game: Game::XIII,
            installs: HashMap::new(),
            install_idx: HashMap::new(),
            add_error: None,
            job: JobRunner::default(),
            modder: crate::modder::ModderState::default(),
            render_state: None,
        };
        app.refresh_installs();
        app
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            render_state: cc.wgpu_render_state.clone(),
            ..Self::default()
        }
    }

    pub fn install(&self) -> Option<&GameInstall> {
        let list = self.installs.get(&self.game)?;
        list.get(self.selected_idx(self.game))
    }

    fn selected_idx(&self, game: Game) -> usize {
        let idx = self.install_idx.get(&game).copied().unwrap_or(0);
        let len = self.installs.get(&game).map_or(0, Vec::len);
        if idx < len { idx } else { 0 }
    }

    pub fn refresh_installs(&mut self) {
        self.installs = Game::ALL
            .into_iter()
            .map(|g| (g, discovery::installs(g)))
            .collect();
        for g in Game::ALL {
            let len = self.installs.get(&g).map_or(0, Vec::len);
            let idx = self.install_idx.entry(g).or_insert(0);
            if *idx >= len {
                *idx = 0;
            }
        }
    }

    fn is_installed(&self, game: Game) -> bool {
        self.installs.get(&game).is_some_and(|v| !v.is_empty())
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Chronostasis");
                ui.separator();
                egui::ComboBox::from_id_salt("game")
                    .selected_text(game_label(self.game, self.is_installed(self.game)))
                    .show_ui(ui, |ui| {
                        for g in Game::ALL {
                            let label = game_label(g, self.is_installed(g));
                            ui.selectable_value(&mut self.game, g, label);
                        }
                    });
                self.install_picker(ui);
                if ui.button("⟳ Rescan").clicked() {
                    self.refresh_installs();
                }
                if ui.button("Add game folder…").clicked() {
                    self.add_install();
                }
                ui.separator();
                ui.label("Modder");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(label) = self.job.running_label() {
                        if let Some((done, total)) = self.job.progress() {
                            ui.add(
                                egui::ProgressBar::new(done as f32 / total as f32)
                                    .desired_width(180.0),
                            );
                        } else {
                            ui.spinner();
                        }
                        ui.label(label.to_string());
                    }
                });
            });
            if let Some(err) = &self.add_error {
                ui.label(egui::RichText::new(err).color(egui::Color32::from_rgb(220, 120, 120)));
            }
        });
    }

    fn install_picker(&mut self, ui: &mut egui::Ui) {
        let game = self.game;
        let count = self.installs.get(&game).map_or(0, Vec::len);
        if count < 2 {
            return;
        }
        let current = self.selected_idx(game);
        let mut chosen = current;
        egui::ComboBox::from_id_salt("install")
            .selected_text("game folder")
            .show_ui(ui, |ui| {
                if let Some(list) = self.installs.get(&game) {
                    for (i, gi) in list.iter().enumerate() {
                        ui.selectable_value(&mut chosen, i, gi.root.display().to_string());
                    }
                }
            });
        if chosen != current {
            self.install_idx.insert(game, chosen);
        }
    }

    fn add_install(&mut self) {
        self.add_error = None;
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        if !discovery::is_game_root(self.game, &dir) {
            self.add_error = Some(format!(
                "{} doesn't look like a {} install.",
                dir.display(),
                self.game.display_name()
            ));
            return;
        }
        let gi = GameInstall::new(self.game, dir.clone());
        if let Err(e) = discovery::register_install(&gi) {
            self.add_error = Some(format!("Couldn't save the game folder: {e}"));
            return;
        }
        self.refresh_installs();
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.job.poll() {
            self.modder.refresh_fs();
        }
        if self.job.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.top_bar(ctx);
        egui::CentralPanel::default().show(ctx, |ui| self.modder_tab(ui));
    }
}

fn game_label(g: Game, installed: bool) -> String {
    format!(
        "{}{}",
        g.display_name(),
        if installed { " (available)" } else { "" }
    )
}
