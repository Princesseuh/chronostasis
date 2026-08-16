use std::collections::HashMap;

use ff13::config::SuiteConfig;
use ff13::{Game, GameInstall, discovery, proxy};

use crate::job::JobRunner;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Player,
    Modder,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayerPage {
    #[default]
    Play,
    Mods,
}

/// Recomputed on a game change, an explicit refresh, and after any job finishes.
#[derive(Default)]
pub struct Status {
    pub installed: bool,
    /// `None` when the exe could not be read.
    pub laa: Option<bool>,
    pub deployed: bool,
    pub ini_present: bool,
    pub unpacked: bool,
}

pub struct App {
    pub tab: Tab,
    pub player_page: PlayerPage,
    pub game: Game,
    pub installs: HashMap<Game, Vec<GameInstall>>,
    pub install_idx: HashMap<Game, usize>,
    pub add_error: Option<String>,
    pub job: JobRunner,

    pub config: SuiteConfig,
    pub config_loaded_for: Option<Game>,
    pub status: Status,
    pub dll_path: String,
    pub dxvk_path: String,
    pub include_en: bool,
    pub include_jp: bool,

    pub setup_want_mods: bool,
    pub setup_want_laa: bool,
    pub unpack_jp: bool,
    /// Shows the setup prompt even when already deployed.
    pub force_setup: bool,
    pub confirm_revert: bool,

    pub modder: crate::modder::ModderState,

    pub mods_mgr: crate::modpacks::ModManager,

    pub render_state: Option<eframe::egui_wgpu::RenderState>,
}

impl Default for App {
    fn default() -> Self {
        let mut app = Self {
            tab: Tab::Player,
            player_page: PlayerPage::default(),
            game: Game::XIII,
            installs: HashMap::new(),
            install_idx: HashMap::new(),
            add_error: None,
            job: JobRunner::default(),
            config: SuiteConfig::default(),
            config_loaded_for: None,
            status: Status::default(),
            dll_path: String::new(),
            dxvk_path: String::new(),
            include_en: true,
            include_jp: true,
            setup_want_mods: false,
            setup_want_laa: true,
            unpack_jp: false,
            force_setup: false,
            confirm_revert: false,
            modder: crate::modder::ModderState::default(),
            mods_mgr: crate::modpacks::ModManager::default(),
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

    /// 0 when unset or out of range.
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
        self.refresh_status();
    }

    pub fn refresh_status(&mut self) {
        let mut status = Status::default();
        if let Some(gi) = self.install() {
            status.installed = true;
            status.laa = gi.is_laa_patched().ok();
            status.deployed = gi
                .bin_dir()
                .map(|b| b.join("d3d9.dll").is_file())
                .unwrap_or(false);
            status.ini_present = proxy::ini_path(gi).map(|p| p.is_file()).unwrap_or(false);
            status.unpacked = gi.is_unpacked();
        }
        self.status = status;
    }

    /// Falls back to defaults when there is no ini to load.
    fn ensure_config_loaded(&mut self) {
        if self.config_loaded_for == Some(self.game) {
            return;
        }
        self.config = self
            .install()
            .and_then(proxy::read_config)
            .unwrap_or_default();
        self.config_loaded_for = Some(self.game);
        self.refresh_status();
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.heading("Chronostasis");
                ui.separator();

                let installed = self.install().is_some();
                let play = egui::Button::new(egui::RichText::new("▶ Play").strong())
                    .fill(egui::Color32::from_rgb(60, 120, 70));
                if ui
                    .add_enabled(installed, play)
                    .on_hover_text("Start the game through Steam (applies the launch options).")
                    .clicked()
                    && let Err(e) = ff13::launch::launch_via_steam(self.game)
                {
                    self.add_error = Some(format!("Couldn't start the game: {e}"));
                }
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
                if ui
                    .button("Add install…")
                    .on_hover_text("Register a non-Steam install folder for the selected game.")
                    .clicked()
                {
                    self.add_install();
                }

                ui.separator();
                ui.selectable_value(&mut self.tab, Tab::Player, "Player");
                ui.selectable_value(&mut self.tab, Tab::Modder, "Modder");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(label) = self.job.running_label() {
                        match self.job.progress() {
                            Some((done, total)) => {
                                ui.add(
                                    egui::ProgressBar::new(done as f32 / total as f32)
                                        .desired_width(180.0)
                                        .text(format!("{done} / {total}")),
                                );
                            }
                            None => {
                                ui.spinner();
                            }
                        }
                        ui.label(label.to_string());
                    }
                });
            });
            if let Some(err) = &self.add_error {
                ui.label(egui::RichText::new(err).color(egui::Color32::from_rgb(220, 120, 120)));
            }
            ui.add_space(2.0);
        });
    }

    fn is_installed(&self, game: Game) -> bool {
        self.installs.get(&game).is_some_and(|v| !v.is_empty())
    }

    fn install_picker(&mut self, ui: &mut egui::Ui) {
        let game = self.game;
        let count = self.installs.get(&game).map_or(0, Vec::len);
        if count < 2 {
            return;
        }
        let current = self.selected_idx(game);
        let label = self
            .installs
            .get(&game)
            .and_then(|v| v.get(current))
            .map(|gi| gi.root.display().to_string())
            .unwrap_or_default();
        let mut chosen = current;
        egui::ComboBox::from_id_salt("install")
            .selected_text(short_path(&label))
            .width(280.0)
            .show_ui(ui, |ui| {
                if let Some(list) = self.installs.get(&game) {
                    for (i, gi) in list.iter().enumerate() {
                        ui.selectable_value(&mut chosen, i, gi.root.display().to_string());
                    }
                }
            });
        if chosen != current {
            self.install_idx.insert(game, chosen);
            self.config_loaded_for = None;
            self.refresh_status();
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
            self.add_error = Some(format!("Couldn't save the install: {e}"));
            return;
        }
        self.refresh_installs();
        if let Some(list) = self.installs.get(&self.game)
            && let Some(i) = list.iter().position(|g| g.root == dir)
        {
            self.install_idx.insert(self.game, i);
        }
        self.config_loaded_for = None;
        self.refresh_status();
    }
}

/// Keeps the last few components.
fn short_path(p: &str) -> String {
    let parts: Vec<&str> = p.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    if parts.len() <= 3 {
        return p.to_string();
    }
    format!("…/{}", parts[parts.len() - 3..].join("/"))
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.job.poll() {
            self.refresh_status();
            self.mods_mgr.mark_dirty();
            self.modder.refresh_fs();
        }
        // Keeps the progress bar animating without input while a job runs.
        if self.job.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        self.top_bar(ctx);
        self.ensure_config_loaded();

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Player => self.player_tab(ui),
            Tab::Modder => self.modder_tab(ui),
        });
    }
}

fn game_label(g: Game, installed: bool) -> String {
    let mark = if installed { " (installed)" } else { "" };
    format!("{}{mark}", g.display_name())
}
