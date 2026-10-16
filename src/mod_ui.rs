//! Installed Tempest registry and local package import; uses the shared worker.
use super::*;

impl App {
    pub(super) fn refresh_mods(&mut self) {
        self.mods_folder = self.settings.game_folder.clone();
        self.mods = if self.mods_folder.is_empty() {
            Ok(Vec::new())
        } else if self.preview {
            self.mods.clone()
        } else {
            mods::list(&PathBuf::from(&self.mods_folder))
        };
    }

    pub(super) fn mods_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, busy: bool) {
        if self.mods_folder != self.settings.game_folder {
            self.refresh_mods();
        }
        let language = self.settings.ui_language;
        let folder = self.settings.game_folder.clone();
        let selected = !folder.is_empty();
        let mut action = None;
        card().show(ui, |ui| {
            ui.label(RichText::new(language.text("Tempest mods")).font(bold_font(16.)));
            ui.add_enabled_ui(!busy && selected, |ui| {
                ui.horizontal(|ui| {
                    if ui.button(language.text("Add mod…")).clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .set_title(language.text("Add mod…"))
                            .add_filter(
                                language.text("Tempest mods"),
                                &["tempest", "zip", "upk", "pck"],
                            )
                            .pick_file()
                    {
                        let game = folder.clone();
                        let replace = self.replace_mod_files;
                        let allow_native = self.allow_native_mods;
                        self.work(ctx, move |_| {
                            mods::import(&PathBuf::from(&game), &path, replace, allow_native)
                                .map(|entries| Work::Mods(game, entries, "Mod added"))
                        });
                    }
                    if ui.button(language.text("Refresh")).clicked() {
                        let game = folder.clone();
                        self.work(ctx, move |_| {
                            mods::list(&PathBuf::from(&game))
                                .map(|entries| Work::Mods(game, entries, "Mods refreshed"))
                        });
                    }
                    if !self.preview
                        && mods::recovery_pending(&PathBuf::from(&folder))
                        && ui.button(language.text("Recover mods")).clicked()
                    {
                        let game = folder.clone();
                        self.work(ctx, move |_| {
                            mods::recover(&PathBuf::from(&game))
                                .map(|entries| Work::Mods(game, entries, "Mods recovered"))
                        });
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(
                        &mut self.replace_mod_files,
                        language.text("Replace existing game files"),
                    );
                    ui.checkbox(
                        &mut self.allow_native_mods,
                        language.text("Allow unverified DLL mods"),
                    );
                });
            });
            if !selected {
                ui.label(
                    RichText::new(language.text("Select a game folder to manage mods"))
                        .color(MUTED),
                );
                return;
            }
            match &self.mods {
                Ok(entries) if entries.is_empty() => {
                    ui.label(RichText::new(language.text("No installed mods")).color(MUTED));
                }
                Ok(entries) => {
                    ui.separator();
                    egui::Grid::new("installed-tempest-mods")
                        .num_columns(5)
                        .spacing([16., 10.])
                        .striped(true)
                        .show(ui, |ui| {
                            for header in ["Name", "Version", "Author", "Status", "Actions"] {
                                ui.label(RichText::new(language.text(header)).color(MUTED));
                            }
                            ui.end_row();
                            for entry in entries {
                                ui.add_sized(
                                    [210., 24.],
                                    egui::Label::new(if entry.name.is_empty() {
                                        &entry.id
                                    } else {
                                        &entry.name
                                    })
                                    .truncate(),
                                )
                                .on_hover_text(format!(
                                    "{}\n{} · {}",
                                    entry.name, entry.id, entry.kind
                                ));
                                ui.add_sized(
                                    [64., 24.],
                                    egui::Label::new(&entry.version).truncate(),
                                );
                                ui.add_sized(
                                    [120., 24.],
                                    egui::Label::new(&entry.author).truncate(),
                                )
                                .on_hover_text(&entry.author);
                                ui.label(language.text(if entry.enabled {
                                    "Enabled"
                                } else {
                                    "Disabled"
                                }));
                                ui.push_id(&entry.id, |ui| {
                                    let response = ui
                                        .add_enabled_ui(
                                            !busy && selected && !entry.managed(),
                                            |ui| {
                                                ui.horizontal(|ui| {
                                                    if ui
                                                        .button(language.text(if entry.enabled {
                                                            "Disable"
                                                        } else {
                                                            "Enable"
                                                        }))
                                                        .clicked()
                                                    {
                                                        action = Some((
                                                            entry.id.clone(),
                                                            Some(!entry.enabled),
                                                        ));
                                                    }
                                                    if ui.button(language.text("Remove")).clicked()
                                                    {
                                                        action = Some((entry.id.clone(), None));
                                                    }
                                                });
                                            },
                                        )
                                        .response;
                                    if entry.managed() {
                                        response.on_disabled_hover_text(
                                            language.text("Manage in Localization or Performance"),
                                        );
                                    }
                                });
                                ui.end_row();
                            }
                        });
                }
                Err(error) => {
                    ui.label(language.text("Unable to read installed mods"));
                    egui::CollapsingHeader::new(language.text("Diagnostics"))
                        .id_salt("mods-list-error")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(error).wrap().selectable(true));
                        });
                }
            }
        });
        if let Some((id, enabled)) = action {
            self.work(ctx, move |_| {
                let game = PathBuf::from(&folder);
                let result = match enabled {
                    Some(enabled) => mods::set_enabled(&game, &id, enabled),
                    None => mods::remove(&game, &id),
                };
                result.map(|entries| {
                    Work::Mods(
                        folder,
                        entries,
                        match enabled {
                            Some(true) => "Mod enabled",
                            Some(false) => "Mod disabled",
                            None => "Mod removed",
                        },
                    )
                })
            });
        }
    }
}
