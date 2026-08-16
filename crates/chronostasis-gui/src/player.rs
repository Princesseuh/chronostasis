use std::path::PathBuf;
use std::sync::atomic::Ordering;

use ff13::archive::{self, Variant};
use ff13::config::{Resolution, SuiteConfig};
use ff13::launch::{ProtonOptions, launch_options_relevant};
use ff13::{laa_patch, proxy};

use crate::app::{App, PlayerPage};

impl App {
    pub(crate) fn player_tab(&mut self, ui: &mut egui::Ui) {
        if self.install().is_none() {
            self.not_installed(ui);
            return;
        }

        if !self.status.deployed {
            self.player_page = PlayerPage::Play;
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.player_page, PlayerPage::Play, "Play");
            let mods = ui.add_enabled(
                self.status.deployed,
                egui::SelectableLabel::new(self.player_page == PlayerPage::Mods, "Nova Modpacks"),
            );
            if mods
                .on_disabled_hover_text("Set up the game first.")
                .clicked()
            {
                self.player_page = PlayerPage::Mods;
            }
        });
        ui.separator();

        match self.player_page {
            PlayerPage::Play => {
                egui::ScrollArea::vertical().show(ui, |ui| self.play_page(ui));
            }
            PlayerPage::Mods => self.mods_page(ui),
        }

        self.confirm_revert_dialog(ui);
    }

    fn play_page(&mut self, ui: &mut egui::Ui) {
        self.setup_card(ui);
        ui.add_space(10.0);
        if self.status.deployed {
            ui.separator();
            ui.add_space(6.0);
            self.settings_section(ui);
            ui.add_space(10.0);
        }
        self.job.show_log(ui);
    }

    fn confirm_revert_dialog(&mut self, ui: &mut egui::Ui) {
        if !self.confirm_revert {
            return;
        }
        let ctx = ui.ctx().clone();
        egui::Window::new("Remove unpacked files?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(&ctx, |ui| {
                ui.label(
                    "This deletes the loose game files created by unpacking and returns the \
                     game to packed mode.",
                );
                ui.label(
                    egui::RichText::new(
                        "Your mods in the mods/ folder and the original game archives are not \
                         affected.",
                    )
                    .weak(),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        self.confirm_revert = false;
                    }
                    let del = egui::Button::new("Delete unpacked files")
                        .fill(egui::Color32::from_rgb(150, 60, 60));
                    if ui.add(del).clicked() {
                        self.confirm_revert = false;
                        self.run_revert(ui);
                    }
                });
            });
    }

    fn not_installed(&mut self, ui: &mut egui::Ui) {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.heading(format!("{} not found", self.game.display_name()));
            ui.label("No install was located in any Steam library.");
            if ui.button("⟳ Rescan Steam libraries").clicked() {
                self.refresh_installs();
            }
        });
    }

    /// The whole first-run flow: a setup prompt before deploy, a ready card after.
    fn setup_card(&mut self, ui: &mut egui::Ui) {
        let gi = self.install().unwrap();
        let game = gi.game.display_name();
        let root = gi.root.display().to_string();
        let busy = self.job.busy();
        let ready = self.status.deployed && !self.force_setup;

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if ready {
                self.ready_card(ui, game, &root, busy);
            } else {
                self.setup_prompt(ui, game, &root, busy);
            }
        });
    }

    fn ready_card(&mut self, ui: &mut egui::Ui, game: &str, root: &str, busy: bool) {
        ui.horizontal(|ui| {
            ui.heading(game);
            ui.label(egui::RichText::new("Ready to play").color(GREEN));
        });
        ui.label(egui::RichText::new(root).weak());
        ui.add_space(6.0);
        if launch_options_relevant() {
            ui.label("In Steam, set this game's Launch Options to:");
            let launch = ProtonOptions::recommended_for(self.game, self.status.laa == Some(true))
                .to_launch_string();
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(egui::RichText::new(&launch).monospace()).wrap());
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(launch.clone());
                }
            });
        } else {
            ui.label("Launch the game from Steam as usual.");
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("⟳ Re-run setup").clicked() {
                self.force_setup = true;
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Undeploy"))
                .on_hover_text("Remove the proxy and restore the original d3d9.dll.")
                .clicked()
            {
                self.run_undeploy(ui);
            }
            if self.game.laa_patch_applies() && self.status.laa == Some(true) {
                ui.label(egui::RichText::new("exe patched for 4GB").color(GREEN));
            }
        });
    }

    fn setup_prompt(&mut self, ui: &mut egui::Ui, game: &str, root: &str, busy: bool) {
        ui.heading(game);
        ui.label(egui::RichText::new(root).weak());
        ui.add_space(6.0);
        if self.status.unpacked {
            ui.label(egui::RichText::new("Already unpacked for mods").color(GREEN));
        } else {
            ui.checkbox(
                &mut self.setup_want_mods,
                "I plan to install mods (unpacks the game; tens of GB, takes a while)",
            );
            if self.setup_want_mods {
                self.language_combo(ui);
            }
        }
        if self.game.laa_patch_applies() {
            let tip = if launch_options_relevant() {
                "Lets the game use up to 4GB of RAM, preventing late-game crashes. \
                 Optional under Proton: the launch options already force it."
            } else {
                "Lets the game use up to 4GB of RAM, preventing late-game crashes."
            };
            ui.checkbox(
                &mut self.setup_want_laa,
                "Patch the exe for 4GB (recommended)",
            )
            .on_hover_text(tip);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("  Set up & play  "))
                .clicked()
            {
                self.run_setup(ui);
            }
            if self.force_setup && ui.button("Cancel").clicked() {
                self.force_setup = false;
            }
        });
    }

    /// Two columns when there is room.
    fn settings_section(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.busy();
        let laa = self.status.laa;

        ui.strong("Settings");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Save")).clicked() {
                self.run_save_ini(ui);
            }
            if ui.button("↺ Reload from disk").clicked()
                && let Some(cfg) = self.install().and_then(proxy::read_config)
            {
                self.config = cfg;
            }
            if ui.button("Reset to defaults").clicked() {
                self.config = SuiteConfig::default();
            }
            ui.label(egui::RichText::new("saved to chronostasis.ini").weak());
        });
        ui.add_space(8.0);

        // Stacked into one column when two would be too narrow to hold the controls.
        if ui.available_width() >= 880.0 {
            ui.columns(2, |cols| {
                let ui = &mut cols[0];
                group_card(ui, "Graphics", |ui| {
                    common_settings_ui(&mut self.config, ui)
                });
                group_card(ui, "Performance", |ui| {
                    perf_settings_ui(&mut self.config, ui)
                });
                group_card(ui, "Controller", |ui| {
                    controller_settings_ui(&mut self.config, ui)
                });

                let ui = &mut cols[1];
                group_card(ui, "Display / render", |ui| {
                    display_settings_ui(&mut self.config, ui)
                });
                group_card(ui, "Game / mods", |ui| {
                    game_mods_settings_ui(&mut self.config, ui)
                });
                group_card(ui, "Proxy / runtime", |ui| {
                    self.proxy_controls(ui, busy, laa)
                });
            });
        } else {
            group_card(ui, "Graphics", |ui| {
                common_settings_ui(&mut self.config, ui)
            });
            group_card(ui, "Performance", |ui| {
                perf_settings_ui(&mut self.config, ui)
            });
            group_card(ui, "Controller", |ui| {
                controller_settings_ui(&mut self.config, ui)
            });
            group_card(ui, "Display / render", |ui| {
                display_settings_ui(&mut self.config, ui)
            });
            group_card(ui, "Game / mods", |ui| {
                game_mods_settings_ui(&mut self.config, ui)
            });
            group_card(ui, "Proxy / runtime", |ui| {
                self.proxy_controls(ui, busy, laa)
            });
        }
    }

    /// The setup card covers all of this for everyone else.
    fn proxy_controls(&mut self, ui: &mut egui::Ui, busy: bool, laa: Option<bool>) {
        ui.label("Proxy DLL (blank = bundled/auto):");
        ui.horizontal(|ui| {
            if ui.button("Browse…").clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .add_filter("DLL", &["dll"])
                    .pick_file()
            {
                self.dll_path = p.display().to_string();
            }
            let w = ui.available_width();
            ui.add(egui::TextEdit::singleline(&mut self.dll_path).desired_width(w));
        });
        ui.label("DXVK d3d9 (optional, blank = auto):");
        ui.horizontal(|ui| {
            if ui.button("Browse…").clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .add_filter("DLL", &["dll"])
                    .pick_file()
            {
                self.dxvk_path = p.display().to_string();
            }
            let w = ui.available_width();
            ui.add(egui::TextEdit::singleline(&mut self.dxvk_path).desired_width(w));
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Deploy proxy"))
                .clicked()
            {
                self.run_deploy(ui);
            }
            if ui
                .add_enabled(!busy && self.status.deployed, egui::Button::new("Undeploy"))
                .clicked()
            {
                self.run_undeploy(ui);
            }
            if self.game.laa_patch_applies() {
                let patch_label = if laa == Some(true) {
                    "Revert LAA patch"
                } else {
                    "Apply LAA patch"
                };
                if ui
                    .add_enabled(!busy, egui::Button::new(patch_label))
                    .clicked()
                {
                    self.run_patch(ui, laa == Some(true));
                }
            }
        });
    }

    /// Sits atop the Mods page, since modpacks require an unpacked game.
    pub(crate) fn loose_file_controls(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.busy();
        if self.status.unpacked {
            ui.label(
                egui::RichText::new("Game is unpacked; loose-file mods are ready.").color(GREEN),
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("Re-unpack…"))
                    .on_hover_text(
                        "Re-extract the original archives over the loose tree (rarely needed).",
                    )
                    .clicked()
                {
                    self.run_unpack(ui);
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("Remove unpacked files…"))
                    .on_hover_text("Delete the loose files and return the game to packed mode.")
                    .clicked()
                {
                    self.confirm_revert = true;
                }
            });
        } else {
            ui.label(
                "Modpacks need loose-file modding (LayeredFS): unpack the game and enable \
                 unpacked mode first.",
            );
            self.language_combo(ui);
            if ui
                .add_enabled(!busy, egui::Button::new("Unpack game for mods…"))
                .on_hover_text("Writes tens of GB and takes a while.")
                .clicked()
            {
                self.run_unpack(ui);
            }
        }
    }

    /// Mirrors the CLI's `install`.
    fn run_setup(&mut self, ui: &mut egui::Ui) {
        let Some(gi) = self.install().cloned() else {
            return;
        };
        let dll_field = self.dll_path.trim().to_string();
        let dxvk = {
            let t = self.dxvk_path.trim();
            (!t.is_empty()).then(|| PathBuf::from(t))
        };
        let want_mods = self.setup_want_mods;
        let want_laa = self.setup_want_laa && self.game.laa_patch_applies();
        let variant = self.unpack_variant();
        let mut config = self.config.clone();
        config.unpacked_mode = want_mods || self.status.unpacked || config.unpacked_mode;
        self.force_setup = false;

        self.job.spawn_tracked(ui.ctx(), "Set up", move |progress| {
            let dll = if dll_field.is_empty() {
                proxy::resolve_proxy_dll(None)?
            } else {
                PathBuf::from(&dll_field)
            };
            let opts = proxy::DeployOptions {
                config,
                dxvk_source: dxvk,
            };
            let report = proxy::deploy(&gi, &dll, &opts)?;

            let mut lines = vec![format!("Installed proxy → {}", report.dll_path.display())];
            if let proxy::DxvkStatus::NotFound = report.dxvk {
                lines.push("WARNING: no DXVK found; the game will crash under Proton.".into());
            }
            if want_laa {
                let msg = match gi.is_laa_patched() {
                    Ok(true) => "LAA: exe already patched for 4GB".into(),
                    _ => match gi.apply_laa_patch() {
                        Ok(o) => format!("LAA: patched the exe for 4GB ({o:?})"),
                        Err(e) => format!("LAA: patch failed ({e}); rely on the launch option"),
                    },
                };
                lines.push(msg);
            }
            if want_mods && !gi.is_unpacked_variant(variant) {
                let data_dir = gi.data_dir();
                let total = archive::prepare_for_modding(&gi, variant, &data_dir, true)?;
                progress.total.store(
                    total.main_files + total.script_files + total.zone_files,
                    Ordering::Relaxed,
                );
                let r = archive::prepare_for_modding_with_progress(
                    &gi,
                    variant,
                    &data_dir,
                    &progress.done,
                )?;
                lines.push(format!(
                    "Unpacked {} files → drop mods into {}",
                    r.main_files + r.script_files + r.zone_files,
                    gi.root.join("mods").display()
                ));
            }
            if launch_options_relevant() {
                let laa_now = gi.is_laa_patched().unwrap_or(false);
                let launch = ProtonOptions::recommended_for(gi.game, laa_now).to_launch_string();
                lines.push(format!("Launch options: {launch}"));
            }
            Ok(lines.join("\n"))
        });
    }

    fn run_unpack(&mut self, ui: &mut egui::Ui) {
        let Some(gi) = self.install().cloned() else {
            return;
        };
        self.config.unpacked_mode = true;
        let config = self.config.clone();
        let variant = self.unpack_variant();
        self.job
            .spawn_tracked(ui.ctx(), "Unpack for mods", move |progress| {
                let data_dir = gi.data_dir();
                let total = archive::prepare_for_modding(&gi, variant, &data_dir, true)?;
                progress.total.store(
                    total.main_files + total.script_files + total.zone_files,
                    Ordering::Relaxed,
                );
                let r = archive::prepare_for_modding_with_progress(
                    &gi,
                    variant,
                    &data_dir,
                    &progress.done,
                )?;
                proxy::write_config(&gi, &config).ok();
                Ok(format!(
                    "Unpacked {} files → drop mods into {}",
                    r.main_files + r.script_files + r.zone_files,
                    gi.root.join("mods").display()
                ))
            });
    }

    fn run_revert(&mut self, ui: &mut egui::Ui) {
        let Some(gi) = self.install().cloned() else {
            return;
        };
        self.config.unpacked_mode = false;
        let config = self.config.clone();
        self.job
            .spawn_tracked(ui.ctx(), "Remove unpacked files", move |progress| {
                let data_dir = gi.data_dir();
                let total = archive::prepare_for_modding(&gi, Variant::U, &data_dir, true)?;
                progress.total.store(
                    total.main_files + total.script_files + total.zone_files,
                    Ordering::Relaxed,
                );
                let removed = archive::revert_unpack_with_progress(&gi, &progress.done)?;
                proxy::write_config(&gi, &config).ok();
                Ok(format!(
                    "Removed {removed} unpacked files; back to packed mode."
                ))
            });
    }

    fn unpack_variant(&self) -> Variant {
        if self.unpack_jp {
            Variant::C
        } else {
            Variant::U
        }
    }

    fn language_combo(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Language:");
            egui::ComboBox::from_id_salt("unpack_lang")
                .selected_text(if self.unpack_jp {
                    "Japanese"
                } else {
                    "English"
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.unpack_jp, false, "English");
                    ui.selectable_value(&mut self.unpack_jp, true, "Japanese");
                });
        })
        .response
        .on_hover_text("Which language build to unpack. Match the language you play the game in.");
    }

    fn run_patch(&mut self, ui: &mut egui::Ui, revert: bool) {
        let Some(gi) = self.install().cloned() else {
            return;
        };
        self.job.spawn(ui.ctx(), "LAA patch", move || {
            if revert {
                let changed = laa_patch::revert_laa_patch(&gi.exe_path())?;
                Ok(if changed {
                    "reverted".into()
                } else {
                    "was not patched".into()
                })
            } else {
                Ok(format!("{:?}", gi.apply_laa_patch()?))
            }
        });
    }

    fn run_deploy(&mut self, ui: &mut egui::Ui) {
        let Some(gi) = self.install().cloned() else {
            return;
        };
        let dll_field = self.dll_path.trim().to_string();
        let dxvk = {
            let t = self.dxvk_path.trim();
            (!t.is_empty()).then(|| PathBuf::from(t))
        };
        let config = self.config.clone();
        self.job.spawn(ui.ctx(), "Deploy proxy", move || {
            let dll = if dll_field.is_empty() {
                proxy::resolve_proxy_dll(None)?
            } else {
                PathBuf::from(&dll_field)
            };
            let opts = proxy::DeployOptions {
                config,
                dxvk_source: dxvk,
            };
            let report = proxy::deploy(&gi, &dll, &opts)?;
            let dxvk_note = match report.dxvk {
                proxy::DxvkStatus::Provisioned(src) => format!("; dxvk.dll ← {}", src.display()),
                proxy::DxvkStatus::AlreadyPresent => "; dxvk.dll already present".into(),
                proxy::DxvkStatus::NotFound => {
                    "; WARNING no DXVK found (game will crash under Proton)".into()
                }
            };
            Ok(format!("→ {}{dxvk_note}", report.dll_path.display()))
        });
    }

    fn run_undeploy(&mut self, ui: &mut egui::Ui) {
        let Some(gi) = self.install().cloned() else {
            return;
        };
        self.job.spawn(ui.ctx(), "Undeploy proxy", move || {
            let restored = proxy::undeploy(&gi)?;
            Ok(if restored {
                "removed, restored previous d3d9.dll".into()
            } else {
                "removed".into()
            })
        });
    }

    fn run_save_ini(&mut self, ui: &mut egui::Ui) {
        let Some(gi) = self.install().cloned() else {
            return;
        };
        let config = self.config.clone();
        self.job.spawn(ui.ctx(), "Save .ini", move || {
            proxy::write_config(&gi, &config)?;
            Ok("written".into())
        });
    }
}

pub(crate) const GREEN: egui::Color32 = egui::Color32::from_rgb(120, 200, 120);

/// Friendly controls over the raw ini fields.
fn common_settings_ui(cfg: &mut SuiteConfig, ui: &mut egui::Ui) {
    egui::Grid::new("common_settings")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Display mode");
            display_mode_ui(cfg, ui);
            ui.end_row();

            ui.label("Resolution");
            optional_resolution(cfg, ui);
            ui.end_row();

            ui.label("Frame rate");
            ui.horizontal(|ui| {
                ui.checkbox(&mut cfg.framerate_uncap, "Uncap");
                ui.add_enabled_ui(cfg.framerate_uncap, |ui| {
                    ui.label("limit");
                    ui.add(egui::DragValue::new(&mut cfg.frame_rate_limit).range(0..=1000));
                    ui.label(egui::RichText::new("(0 = none)").weak());
                });
            });
            ui.end_row();

            ui.label("Vsync");
            vsync_ui(cfg, ui);
            ui.end_row();

            ui.label("Anisotropic filtering");
            ui.add(egui::Slider::new(&mut cfg.anisotropic, 0..=16));
            ui.end_row();
        });
}

fn group_card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.strong(title);
        ui.add_space(2.0);
        add(ui);
    });
}

fn perf_settings_ui(cfg: &mut SuiteConfig, ui: &mut egui::Ui) {
    ui.checkbox(
        &mut cfg.facial_anim_fix,
        "Fix fast facial animation in cutscenes",
    )
    .on_hover_text(
        "Keeps blinking and mouth movement at the right speed. Without it, faces \
             animate far too fast at high frame rates. Best left on.",
    );
    ui.checkbox(&mut cfg.experimental_mipmap_fix, "Experimental mipmap fix");
    optional_lod_bias(cfg, ui);
}

fn controller_settings_ui(cfg: &mut SuiteConfig, ui: &mut egui::Ui) {
    ui.checkbox(&mut cfg.controller_scan_fix, "Controller scan-stutter fix");
    ui.add_enabled_ui(cfg.controller_scan_fix, |ui| {
        ui.checkbox(
            &mut cfg.controller_hotplug,
            "Hotplug detect (needs scan fix)",
        );
    });
    ui.checkbox(&mut cfg.controller_trigger_fix, "Xbox One trigger-axis fix");
    ui.checkbox(&mut cfg.vibration, "Vibration");
    ui.horizontal(|ui| {
        ui.label("Vibration strength:");
        ui.add(
            egui::DragValue::new(&mut cfg.vibration_strength)
                .speed(0.1)
                .range(0.0..=10.0),
        );
    });
}

fn display_settings_ui(cfg: &mut SuiteConfig, ui: &mut egui::Ui) {
    ui.checkbox(
        &mut cfg.device_fixes,
        "Device fixes (resolution/scissor/UI)",
    );
    ui.checkbox(
        &mut cfg.startup_overlay,
        "Startup overlay (confirm proxy active)",
    );
    ui.checkbox(&mut cfg.triple_buffering, "Triple buffering");
    ui.horizontal(|ui| {
        ui.label("Fullscreen refresh:");
        ui.add(egui::DragValue::new(&mut cfg.fullscreen_refresh).range(-1..=360));
    });
    ui.horizontal(|ui| {
        ui.label("Multisample:");
        ui.add(egui::DragValue::new(&mut cfg.multisample).range(0..=16));
    });
    ui.horizontal(|ui| {
        ui.label("Swap effect:");
        ui.add(egui::DragValue::new(&mut cfg.swap_effect).range(-1..=5));
    });
    ui.checkbox(&mut cfg.always_active, "Always active");
    ui.checkbox(&mut cfg.hide_cursor, "Hide cursor");
    ui.checkbox(&mut cfg.top_most, "Top-most window");
}

fn game_mods_settings_ui(cfg: &mut SuiteConfig, ui: &mut egui::Ui) {
    const LANGS: [&str; 8] = ["en", "fr", "de", "it", "es", "jp", "cn", "kr"];

    ui.checkbox(
        &mut cfg.unpacked_mode,
        "Unpacked mode (read loose modded files)",
    );
    ui.checkbox(
        &mut cfg.texture_mods,
        "Texture mods (LayeredFS; needs unpacked mode)",
    );
    ui.add_enabled_ui(cfg.texture_mods, |ui| {
        ui.checkbox(
            &mut cfg.unsafe_gui_overlay,
            "Overlay GUI texture containers (unsafe)",
        )
        .on_hover_text(
            "Also file-overlay .imgb/.xgr GUI containers. Only safe for same-dimension \
                 swaps; a higher-res container resizes the HUD (use a .dds for HD GUI instead).",
        );
    });
    ui.checkbox(&mut cfg.debug_mode, "Debug mode");
    ui.checkbox(&mut cfg.confirm_exit, "Confirm-exit dialog");

    let mut force_lang = cfg.text_language.is_some();
    if ui
        .checkbox(&mut force_lang, "Force text language")
        .changed()
    {
        cfg.text_language = force_lang.then(|| "en".to_string());
    }
    if let Some(lang) = &mut cfg.text_language {
        egui::ComboBox::from_id_salt("lang_combo")
            .selected_text(lang.clone())
            .show_ui(ui, |ui| {
                for l in LANGS {
                    ui.selectable_value(lang, l.to_string(), l);
                }
            });
    }
}

fn display_mode_ui(cfg: &mut SuiteConfig, ui: &mut egui::Ui) {
    let current = match (cfg.borderless, cfg.force_windowed) {
        (true, _) => "Borderless",
        (_, true) => "Windowed",
        _ => "Fullscreen",
    };
    egui::ComboBox::from_id_salt("display_mode")
        .selected_text(current)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(current == "Fullscreen", "Fullscreen")
                .clicked()
            {
                cfg.borderless = false;
                cfg.force_windowed = false;
            }
            if ui
                .selectable_label(current == "Borderless", "Borderless")
                .clicked()
            {
                cfg.borderless = true;
                cfg.force_windowed = false;
            }
            if ui
                .selectable_label(current == "Windowed", "Windowed")
                .clicked()
            {
                cfg.borderless = false;
                cfg.force_windowed = true;
            }
        });
}

/// Friendly 3-way over `present_interval`.
fn vsync_ui(cfg: &mut SuiteConfig, ui: &mut egui::Ui) {
    let current = match cfg.present_interval {
        1 => "On",
        -1 => "Off",
        _ => "Game default",
    };
    egui::ComboBox::from_id_salt("vsync")
        .selected_text(current)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut cfg.present_interval, 1, "On");
            ui.selectable_value(&mut cfg.present_interval, -1, "Off");
            ui.selectable_value(&mut cfg.present_interval, 0, "Game default");
        });
}

fn optional_lod_bias(cfg: &mut SuiteConfig, ui: &mut egui::Ui) {
    let mut over = cfg.lod_bias.is_some();
    if ui.checkbox(&mut over, "Override LOD bias").changed() {
        cfg.lod_bias = over.then_some(0.0);
    }
    if let Some(b) = &mut cfg.lod_bias {
        ui.add(egui::DragValue::new(b).speed(0.05).range(-4.0..=4.0));
    }
}

fn optional_resolution(cfg: &mut SuiteConfig, ui: &mut egui::Ui) {
    let mut force = cfg.render_resolution.is_some();
    if ui.checkbox(&mut force, "Force render resolution").changed() {
        cfg.render_resolution = force.then_some(Resolution {
            width: 2560,
            height: 1440,
        });
    }
    if let Some(res) = &mut cfg.render_resolution {
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut res.width).range(640..=7680));
            ui.label("×");
            ui.add(egui::DragValue::new(&mut res.height).range(480..=4320));
        });
    }
}
