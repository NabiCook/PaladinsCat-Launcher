#![cfg_attr(windows, windows_subsystem = "windows")]

mod graphics_device;
mod i18n;
mod language_menu;
mod mod_ui;
mod preview;
mod vm_renderer;
use i18n::UiLanguage;
use paladinscat_launcher::{
    cache, engine, game_discovery, ime, launch, mods, performance, self_update, sources,
};
use sources::{LOCALE_REPOSITORY as REPOSITORY, catalog_url};

use egui::{Color32, FontFamily, FontId, RichText};
use engine::{Action, Cached, Release};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    thread,
};

const BG: Color32 = Color32::from_rgb(27, 27, 31);
const PANEL: Color32 = Color32::from_rgb(32, 33, 39);
const TEXT: Color32 = Color32::from_rgb(223, 223, 214);
const MUTED: Color32 = Color32::from_rgb(152, 152, 159);
const ACCENT: Color32 = Color32::from_rgb(100, 186, 177);
const DEFAULT_WINDOW_SIZE: [f32; 2] = [880., 660.];

#[derive(Clone, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    ui_language: UiLanguage,
    game_folder: String,
    repository: String,
    catalog_url: String,
    #[serde(default)]
    prepared_translation: String,
    #[serde(default)]
    prepared_ime: String,
    #[serde(default)]
    prepared_performance: String,
    #[serde(default)]
    performance_enabled: bool,
    #[serde(default)]
    rollback_input: bool,
    #[serde(default = "default_language")]
    launch_language: String,
    #[serde(default = "default_homedir")]
    homedir: String,
    #[serde(default)]
    launch_platform: launch::LaunchPlatform,
}
fn default_language() -> String {
    launch::DEFAULT_LANGUAGE.into()
}
fn default_homedir() -> String {
    launch::DEFAULT_PROFILE.into()
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            ui_language: UiLanguage::default(),
            game_folder: String::new(),
            repository: REPOSITORY.into(),
            catalog_url: catalog_url(REPOSITORY),
            prepared_translation: String::new(),
            prepared_ime: String::new(),
            prepared_performance: String::new(),
            performance_enabled: false,
            rollback_input: false,
            launch_language: default_language(),
            homedir: default_homedir(),
            launch_platform: launch::LaunchPlatform::default(),
        }
    }
}
enum Event {
    Progress(String),
    Done(std::result::Result<Work, String>),
    CleanupWarning(String),
    // Deliver completion and its optional warning atomically. Done processing
    // drops the channel receiver, so a later separate send could lose a warning.
    Completion(std::result::Result<Work, String>, Option<String>),
}
enum Work {
    Mods(String, Vec<mods::ModInfo>, &'static str),
    Startup(Startup),
    Catalog(engine::Catalog),
    Package(Box<Cached>),
    ImePackage(Box<ime::Cached>),
    PerformancePackage(Box<performance::Cached>),
    DownloadsCleared(engine::Result<()>),
    Launch(launch::LaunchResult),
    Message(String),
}
struct Startup {
    prepared: PreparedPaths,
    catalog: engine::Result<engine::Catalog>,
    translation: Option<engine::Result<Box<Cached>>>,
    ime: Option<engine::Result<Box<ime::Cached>>>,
    performance: Option<engine::Result<Box<performance::Cached>>>,
    performance_release: Option<Box<performance::Release>>,
}
#[derive(Default)]
struct PreparedPaths {
    translation: String,
    ime: String,
    performance: String,
}
impl Startup {
    fn load(settings: &Settings, cache: &std::path::Path, progress: engine::Progress<'_>) -> Self {
        let translation = (!settings.prepared_translation.is_empty()).then(|| {
            engine::import(
                &PathBuf::from(&settings.prepared_translation),
                cache,
                progress,
            )
            .map(Box::new)
        });
        let ime = (!settings.prepared_ime.is_empty())
            .then(|| ime::import(&PathBuf::from(&settings.prepared_ime), cache).map(Box::new));
        let catalog = engine::check_catalog(&settings.source_url());
        let performance = (!settings.prepared_performance.is_empty()).then(|| {
            performance::import(&PathBuf::from(&settings.prepared_performance), cache).map(Box::new)
        });
        Self {
            prepared: PreparedPaths {
                translation: settings.prepared_translation.clone(),
                ime: settings.prepared_ime.clone(),
                performance: settings.prepared_performance.clone(),
            },
            catalog,
            translation,
            ime,
            performance,
            performance_release: performance::check().ok().map(Box::new),
        }
    }
}
impl Settings {
    fn load() -> Self {
        std::fs::read(engine::data_dir().join("settings.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
    fn source_url(&self) -> String {
        if self.catalog_url.trim().is_empty() {
            catalog_url(&self.repository)
        } else {
            self.catalog_url.trim().into()
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
enum PackageTab {
    Mods,
    Translation,
    // Korean-only child of Localization, never a top-level menu option.
    Ime,
    Performance,
}
enum ImeInstallState {
    NoGame,
    Disabled,
    Enabled(Box<ime::Package>),
    Invalid(String),
}
impl ImeInstallState {
    fn inspect(folder: &str) -> Self {
        if folder.is_empty() {
            return Self::NoGame;
        }
        match ime::installed_package(&PathBuf::from(folder)) {
            Ok(Some(package)) => Self::Enabled(Box::new(package)),
            Ok(None) => Self::Disabled,
            Err(error) => Self::Invalid(error),
        }
    }
    fn label(&self) -> String {
        match self {
            Self::NoGame => "No game folder selected".into(),
            Self::Disabled => "Disabled".into(),
            Self::Enabled(p) => format!("Enabled · {} · {}", p.version, p.game_build),
            Self::Invalid(error) => format!("Unavailable: {error}"),
        }
    }
    fn enabled(&self) -> bool {
        matches!(self, Self::Enabled(_))
    }
    fn can_toggle(&self, prepared: bool, recovery: bool) -> bool {
        !recovery && (self.enabled() || (matches!(self, Self::Disabled) && prepared))
    }
    fn toggle_action(
        &self,
        requested: bool,
        prepared: bool,
        recovery: bool,
    ) -> Option<ime::Action> {
        if !self.can_toggle(prepared, recovery) {
            return None;
        }
        match (self, requested) {
            (Self::Disabled, true) => Some(ime::Action::Apply),
            (Self::Enabled(_), false) => Some(ime::Action::Remove),
            _ => None,
        }
    }
    fn update_available(&self, prepared: Option<&ime::Cached>) -> bool {
        matches!((self, prepared), (Self::Enabled(installed), Some(prepared)) if **installed != prepared.package)
    }
}
struct App {
    mods: engine::Result<Vec<mods::ModInfo>>,
    mods_folder: String,
    replace_mod_files: bool,
    allow_native_mods: bool,
    settings: Settings,
    discovered_games: Vec<game_discovery::Installation>,
    discovery: Option<Receiver<GameDiscovery>>,
    releases: Vec<Release>,
    release_index: usize,
    language: String,
    selected: Option<Cached>,
    receiver: Option<Receiver<Event>>,
    status: String,
    status_parts: Vec<String>,
    launch_log: Option<PathBuf>,
    installed: String,
    ime_releases: Vec<ime::Release>,
    ime_index: usize,
    ime_selected: Option<ime::Cached>,
    ime_installed: ImeInstallState,
    error: bool,
    logo: egui::TextureHandle,
    preview: bool,
    preview_repository_open: bool,
    tab: PackageTab,
    catalog_status: String,
    catalog_connected: bool,
    performance_selected: Option<performance::Cached>,
    performance_release: Option<Box<performance::Release>>,
    launcher_update: LauncherUpdate,
    update_receiver: Option<Receiver<UpdateEvent>>,
    update_frames: u8,
    update_notice: Option<String>,
}

enum LauncherUpdate {
    Idle,
    Checking,
    Checked(self_update::Check),
    Downloading(u8),
    Restarting,
    Failed(String),
}
enum UpdateEvent {
    Checked(engine::Result<self_update::Check>),
    Progress(u8),
    Armed(engine::Result<()>),
    Cleanup(engine::Result<Option<String>>),
}

struct GameDiscovery {
    requested_folder: String,
    installations: Vec<game_discovery::Installation>,
}

fn configure(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".into(),
        egui::FontData::from_static(include_bytes!("../assets/Inter-Regular.ttf")).into(),
    );
    fonts.font_data.insert(
        "Inter Bold".into(),
        egui::FontData::from_static(include_bytes!("../assets/Inter-Bold.ttf")).into(),
    );
    let mut fallback = Vec::new();
    // Windows supplies Korean glyphs; Inter remains the primary face.
    let windows_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:/Windows"));
    if let Ok(data) = std::fs::read(windows_root.join("Fonts/malgun.ttf")) {
        fonts
            .font_data
            .insert("Korean".into(), egui::FontData::from_owned(data).into());
        fallback.push("Korean".to_owned());
    }
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, "Inter".into());
    proportional.extend(fallback.clone());
    let mut bold = vec!["Inter Bold".into()];
    bold.extend(fallback);
    fonts
        .families
        .insert(FontFamily::Name("InterBold".into()), bold);
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = Color32::from_rgb(22, 22, 24);
    style.visuals.selection.bg_fill = ACCENT.gamma_multiply(0.35);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(40, 42, 48);
    style.visuals.widgets.inactive.bg_stroke =
        egui::Stroke::new(1_f32, Color32::from_rgb(60, 63, 68));
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(53, 59, 63);
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1_f32, ACCENT);
    style.spacing.item_spacing = egui::vec2(8., 10.);
    style.spacing.button_padding = egui::vec2(14., 10.);
    for key in [egui::TextStyle::Body, egui::TextStyle::Button] {
        style.text_styles.insert(key, FontId::proportional(14.));
    }
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(12.));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, bold_font(20.));
    ctx.set_style_of(egui::Theme::Dark, style);
}
fn bold_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("InterBold".into()))
}
fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .corner_radius(10)
        .inner_margin(18)
        .stroke(egui::Stroke::new(1_f32, Color32::from_rgb(60, 63, 68)))
}
impl App {
    fn new(ctx: &egui::Context, preview: bool) -> Self {
        configure(ctx);
        let image = image::load_from_memory(include_bytes!("../assets/paladinscat.png"))
            .expect("Embedded logo")
            .to_rgba8();
        let logo = ctx.load_texture(
            "PaladinsCat",
            egui::ColorImage::from_rgba_unmultiplied(
                [image.width() as usize, image.height() as usize],
                image.as_raw(),
            ),
            egui::TextureOptions::LINEAR,
        );
        let settings = if preview {
            Settings::default()
        } else {
            Settings::load()
        };
        let mut app = Self {
            mods: Ok(Vec::new()),
            mods_folder: String::new(),
            replace_mod_files: false,
            allow_native_mods: false,
            settings,
            discovered_games: Vec::new(),
            discovery: None,
            releases: Vec::new(),
            release_index: 0,
            language: "KOR".into(),
            selected: None,
            receiver: None,
            status: "Ready".into(),
            status_parts: Vec::new(),
            launch_log: None,
            installed: if preview {
                "Not inspected in CPU preview".into()
            } else {
                "No game folder selected".into()
            },
            ime_releases: Vec::new(),
            ime_index: 0,
            ime_selected: None,
            // Offscreen previews use a disabled fixture, never a live receipt.
            ime_installed: if preview {
                ImeInstallState::Disabled
            } else {
                ImeInstallState::NoGame
            },
            error: false,
            logo,
            preview,
            preview_repository_open: false,
            tab: PackageTab::Performance,
            catalog_status: "Catalog not checked".into(),
            catalog_connected: false,
            performance_selected: None,
            performance_release: None,
            launcher_update: LauncherUpdate::Idle,
            update_receiver: None,
            update_frames: 0,
            update_notice: None,
        };
        if !preview {
            app.refresh_installed();
            app.discover_games(ctx);
            app.startup(ctx);
        }
        app
    }
    fn persist(&mut self) {
        if self.preview {
            return;
        }
        let dir = engine::data_dir();
        let result = std::fs::create_dir_all(&dir)
            .map_err(|e| e.to_string())
            .and_then(|_| engine::save_settings(&dir.join("settings.json"), &self.settings));
        if let Err(error) = result {
            self.error = true;
            self.status = format!("Could not save settings: {error}");
            self.status_parts.clear();
        }
    }
    fn check_launcher_update(&mut self, ctx: &egui::Context) {
        if self.preview || self.update_receiver.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.update_receiver = Some(receiver);
        self.launcher_update = LauncherUpdate::Checking;
        let repaint = ctx.clone();
        thread::spawn(move || {
            let _ = sender.send(UpdateEvent::Checked(self_update::check()));
            repaint.request_repaint();
        });
    }
    fn install_launcher_update(&mut self, ctx: &egui::Context, release: self_update::Release) {
        if self.preview || self.receiver.is_some() || self.update_receiver.is_some() {
            return;
        }
        self.persist();
        if self.error && self.status.starts_with("Could not save settings:") {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.update_receiver = Some(receiver);
        self.launcher_update = LauncherUpdate::Downloading(0);
        let repaint = ctx.clone();
        thread::spawn(move || {
            let result = self_update::install(release, &|percent| {
                let _ = sender.send(UpdateEvent::Progress(percent));
                repaint.request_repaint();
            });
            let _ = sender.send(UpdateEvent::Armed(result));
            repaint.request_repaint();
        });
    }
    fn receive_launcher_update(&mut self, ctx: &egui::Context) {
        let events = self
            .update_receiver
            .as_ref()
            .map(|r| r.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            match event {
                UpdateEvent::Checked(result) => {
                    self.update_receiver = None;
                    self.launcher_update = match result {
                        Ok(result) => LauncherUpdate::Checked(result),
                        Err(error) => LauncherUpdate::Failed(error),
                    };
                }
                UpdateEvent::Progress(percent) => {
                    self.launcher_update = LauncherUpdate::Downloading(percent)
                }
                UpdateEvent::Armed(result) => {
                    self.update_receiver = None;
                    match result {
                        Ok(()) => {
                            self.launcher_update = LauncherUpdate::Restarting;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        Err(error) => self.launcher_update = LauncherUpdate::Failed(error),
                    }
                }
                UpdateEvent::Cleanup(result) => {
                    self.update_receiver = None;
                    match result {
                        Ok(Some(error)) => {
                            self.update_notice =
                                Some(format!("Launcher update rolled back: {error}"));
                        }
                        Err(error) => {
                            self.update_notice = Some(format!("Launcher update cleanup: {error}"))
                        }
                        Ok(None) => {}
                    }
                    self.check_launcher_update(ctx);
                }
            }
        }
    }
    fn launcher_update_control(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, busy: bool) {
        let language = self.settings.ui_language;
        let current = concat!("v", env!("CARGO_PKG_VERSION"));
        let (label, tip, enabled) = match &self.launcher_update {
            LauncherUpdate::Checked(self_update::Check::Available(release)) => (
                language.text(&format!("Update v{}", release.version)),
                if self.preview || std::env::args_os().len() == 1 {
                    "Download and restart launcher"
                } else {
                    "Developer launch requires its original restart procedure"
                },
                !busy && (self.preview || std::env::args_os().len() == 1),
            ),
            LauncherUpdate::Downloading(percent) => (
                format!("{} {percent}%", language.text("Updating…")),
                "Downloading launcher update",
                false,
            ),
            LauncherUpdate::Restarting => {
                (language.text("Restarting…"), "Restarting launcher", false)
            }
            LauncherUpdate::Checking => (current.into(), "Checking launcher updates…", false),
            LauncherUpdate::Checked(self_update::Check::NotPublished) => {
                (current.into(), "No launcher release published", true)
            }
            LauncherUpdate::Failed(_) => (
                language.text("Retry update"),
                "Launcher update unavailable; see Diagnostics",
                !busy,
            ),
            _ => (current.into(), "Check for launcher updates", !busy),
        };
        if ui
            .add_enabled(
                enabled && self.update_receiver.is_none(),
                egui::Button::new(label).small(),
            )
            .on_hover_text(language.text(tip))
            .clicked()
        {
            if let LauncherUpdate::Checked(self_update::Check::Available(release)) =
                &self.launcher_update
            {
                self.install_launcher_update(ctx, release.clone());
            } else {
                self.check_launcher_update(ctx);
            }
        }
    }
    fn refresh_installed(&mut self) {
        self.refresh_mods();
        if self.settings.game_folder.is_empty() {
            self.installed = "No game folder selected".into();
            self.ime_installed = ImeInstallState::NoGame;
            return;
        }
        self.installed = engine::installed(&PathBuf::from(&self.settings.game_folder))
            .unwrap_or_else(|e| format!("Receipt unavailable: {e}"));
        self.ime_installed = ImeInstallState::inspect(&self.settings.game_folder);
    }
    fn work(
        &mut self,
        ctx: &egui::Context,
        task: impl FnOnce(&dyn Fn(String)) -> engine::Result<Work> + Send + 'static,
    ) {
        if self.receiver.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.error = false;
        self.status = "Preparing…".into();
        self.status_parts.clear();
        self.launch_log = None;
        let repaint = ctx.clone();
        thread::spawn(move || {
            let progress = |message| {
                let _ = sender.send(Event::Progress(message));
                repaint.request_repaint();
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task(&progress)))
                .unwrap_or_else(|_| {
                    Err("Operation interrupted; use Recover if a transaction is pending".into())
                });
            let cleanup_warning = cache::take_cleanup_warning();
            let _ = sender.send(Event::Completion(result, cleanup_warning));
            repaint.request_repaint();
        });
    }
    fn receive(&mut self) {
        match self.discovery.as_ref().map(|receiver| receiver.try_recv()) {
            Some(Ok(result)) => {
                self.discovery = None;
                self.accept_discovery(result);
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => self.discovery = None,
            _ => {}
        }
        let events: Vec<_> = self
            .receiver
            .as_ref()
            .map(|r| {
                r.try_iter()
                    .flat_map(|event| match event {
                        Event::Completion(result, warning) => {
                            let mut events = vec![Event::Done(result)];
                            events.extend(warning.map(Event::CleanupWarning));
                            events
                        }
                        event => vec![event],
                    })
                    .collect()
            })
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Progress(message) => {
                    self.status = message;
                    self.status_parts.clear();
                }
                Event::CleanupWarning(message) => {
                    self.status_parts
                        .push(format!("Download cache cleanup: {message}"));
                }
                Event::Completion(..) => unreachable!("Worker completion was expanded above"),
                Event::Done(result) => {
                    self.receiver = None;
                    self.status_parts.clear();
                    self.launch_log = None;
                    match result {
                        Ok(Work::Startup(startup)) => {
                            let mut failures = Vec::new();
                            let mut invalidated = false;
                            match startup.catalog {
                                Ok(catalog) => self.accept_catalog(catalog),
                                Err(error) => {
                                    self.catalog_status = "Catalog unavailable".into();
                                    self.catalog_connected = false;
                                    failures.push(format!("GitHub catalog: {error}"));
                                }
                            }
                            if let Some(result) = startup.translation {
                                match result {
                                    Ok(package) => self.accept_translation(*package),
                                    Err(error) => {
                                        invalidated |= self.forget_stale_prepared(
                                            PackageTab::Translation,
                                            &startup.prepared.translation,
                                        );
                                        failures.push(format!("Saved translation: {error}"))
                                    }
                                }
                            }
                            if let Some(result) = startup.ime {
                                match result {
                                    Ok(package) => self.accept_ime(*package),
                                    Err(error) => {
                                        invalidated |= self.forget_stale_prepared(
                                            PackageTab::Ime,
                                            &startup.prepared.ime,
                                        );
                                        failures.push(format!("Saved IME: {error}"));
                                    }
                                }
                            }
                            self.performance_release = startup.performance_release;
                            if let Some(result) = startup.performance {
                                match result {
                                    Ok(package) => self.accept_performance(*package),
                                    Err(error) => {
                                        invalidated |= self.forget_stale_prepared(
                                            PackageTab::Performance,
                                            &startup.prepared.performance,
                                        );
                                        failures.push(format!("Saved performance package: {error}"))
                                    }
                                }
                            }
                            if invalidated {
                                self.persist();
                                if self.error
                                    && self.status.starts_with("Could not save settings: ")
                                {
                                    failures.push(self.status.clone());
                                }
                            }
                            self.error = !failures.is_empty();
                            self.status_parts = failures.clone();
                            self.status = if failures.is_empty() {
                                "Ready".into()
                            } else {
                                failures.join("; ")
                            };
                        }
                        Ok(Work::Mods(folder, entries, status)) => {
                            if folder == self.settings.game_folder {
                                self.mods_folder = folder;
                                self.mods = Ok(entries);
                            }
                            self.status = status.into();
                        }
                        Ok(Work::Catalog(catalog)) => {
                            self.accept_catalog(catalog);
                            self.status = "Catalog refreshed".into();
                        }
                        Ok(Work::Package(package)) => {
                            self.accept_translation(*package);
                            self.status =
                                "Package downloaded/imported and verified. Ready to apply.".into();
                        }
                        Ok(Work::ImePackage(package)) => {
                            self.accept_ime(*package);
                            self.status = "IME package verified. Ready to apply.".into();
                        }
                        Ok(Work::PerformancePackage(package)) => {
                            self.accept_performance(*package);
                            self.status = "Performance package ready".into();
                        }
                        Ok(Work::DownloadsCleared(result)) => {
                            // A partial clear may invalidate any cached package. Drop
                            // all prepared paths even when the backend reports an error.
                            self.selected = None;
                            self.ime_selected = None;
                            self.performance_selected = None;
                            self.settings.prepared_translation.clear();
                            self.settings.prepared_ime.clear();
                            self.settings.prepared_performance.clear();
                            self.error = result.is_err();
                            self.status = match result {
                                Ok(()) => "Downloads cleared".into(),
                                Err(error) => format!("Could not clear downloads: {error}"),
                            };
                            self.persist();
                        }
                        Ok(Work::Launch(result)) => {
                            self.status = if result.game_pid.is_some() {
                                "Game started"
                            } else {
                                "Launch requested"
                            }
                            .into();
                            self.status_parts = vec![
                                result.status,
                                format!("Launch log: {}", result.log.display()),
                            ];
                            self.launch_log = Some(result.log);
                        }
                        Ok(Work::Message(message)) => self.status = message,
                        Err(error) => {
                            if self.catalog_status == "Connecting to GitHub…" {
                                self.catalog_status = "Catalog unavailable".into();
                                self.catalog_connected = false;
                            }
                            self.error = true;
                            self.status = error;
                        }
                    }
                    if !self.preview {
                        self.refresh_installed();
                    }
                }
            }
        }
    }
    fn accept_catalog(&mut self, catalog: engine::Catalog) {
        self.catalog_connected = true;
        self.releases = catalog.packages;
        self.ime_releases = catalog.ime_packages;
        self.release_index = 0;
        self.ime_index = 0;
        if let Some(release) = self.releases.first() {
            self.language = release.locale.clone();
        }
        self.catalog_status = if self.releases.is_empty() && self.ime_releases.is_empty() {
            "Connected · no downloadable packages published".into()
        } else {
            format!(
                "Connected · {} translations · {} IME packages",
                self.releases.len(),
                self.ime_releases.len()
            )
        };
    }
    fn accept_translation(&mut self, package: Cached) {
        self.language = package.package.locale.clone();
        self.settings.prepared_translation =
            package.directory.join("bundle.json").display().to_string();
        self.selected = Some(package);
        self.persist();
    }
    fn accept_ime(&mut self, package: ime::Cached) {
        self.settings.prepared_ime = package
            .directory
            .join("ime-package.json")
            .display()
            .to_string();
        self.ime_selected = Some(package);
        self.persist();
    }
    fn startup(&mut self, ctx: &egui::Context) {
        let settings = self.settings.clone();
        let cache = engine::data_dir();
        self.catalog_status = "Connecting to GitHub…".into();
        self.catalog_connected = false;
        self.work(ctx, move |progress| {
            Ok(Work::Startup(Startup::load(&settings, &cache, progress)))
        });
    }
    fn discover_games(&mut self, ctx: &egui::Context) {
        if self.discovery.is_some() || self.preview {
            return;
        }
        let requested_folder = self.settings.game_folder.clone();
        let (sender, receiver) = mpsc::channel();
        self.discovery = Some(receiver);
        let repaint = ctx.clone();
        thread::spawn(move || {
            let installations = game_discovery::discover();
            let _ = sender.send(GameDiscovery {
                requested_folder,
                installations,
            });
            repaint.request_repaint();
        });
    }
    fn accept_discovery(&mut self, result: GameDiscovery) {
        // A result arriving after Browse/typing must never replace that choice.
        if result.requested_folder == self.settings.game_folder
            && let Some(installation) = game_discovery::automatic_selection(
                &self.settings.game_folder,
                &result.installations,
            )
        {
            self.settings.game_folder = installation.path.display().to_string();
            self.persist();
            self.refresh_installed();
        }
        self.discovered_games = result.installations;
    }
    fn accept_performance(&mut self, package: performance::Cached) {
        self.settings.prepared_performance =
            performance::manifest_path(&package).display().to_string();
        self.performance_selected = Some(package);
        self.persist();
    }
    fn launch_options(&self) -> launch::Options {
        launch::Options {
            game_root: PathBuf::from(&self.settings.game_folder),
            language: self.settings.launch_language.clone(),
            homedir: self.settings.homedir.clone(),
            platform: self.settings.launch_platform,
            performance: self
                .settings
                .performance_enabled
                .then(|| self.performance_selected.clone())
                .flatten(),
            rollback_input: self.settings.performance_enabled && self.settings.rollback_input,
        }
    }
    fn play(&mut self, ctx: &egui::Context) {
        if self.settings.performance_enabled && self.performance_selected.is_none() {
            self.error = true;
            self.status = "Download the selected performance package before launch".into();
            self.status_parts.clear();
            return;
        }
        let options = self.launch_options();
        let logs = engine::data_dir().join("launch-logs");
        self.work(ctx, move |_| {
            launch::start(&options, &logs).map(Work::Launch)
        });
    }
    fn source_url(&self) -> String {
        self.settings.source_url()
    }
    fn action(&mut self, ctx: &egui::Context, action: Action) {
        let game = PathBuf::from(&self.settings.game_folder);
        let selected = self.selected.clone();
        let cache = engine::data_dir();
        self.work(ctx, move |progress| {
            engine::operate(action, &game, selected, &cache, progress).map(Work::Message)
        });
    }
    fn ime_action(&mut self, ctx: &egui::Context, action: ime::Action) {
        let game = PathBuf::from(&self.settings.game_folder);
        let selected = self.ime_selected.clone();
        self.work(ctx, move |progress| {
            ime::operate(action, &game, selected, progress).map(Work::Message)
        });
    }
    fn check_updates(&mut self, ctx: &egui::Context) {
        let url = self.source_url();
        self.catalog_status = "Connecting to GitHub…".into();
        self.catalog_connected = false;
        self.persist();
        self.work(ctx, move |_| engine::check_catalog(&url).map(Work::Catalog));
    }
    fn invalidate_catalog(&mut self) {
        self.catalog_connected = false;
        self.catalog_status = "Catalog not checked".into();
        self.releases.clear();
        self.ime_releases.clear();
        self.release_index = 0;
        self.ime_index = 0;
    }
    fn forget_stale_prepared(&mut self, tab: PackageTab, expected: &str) -> bool {
        if expected.is_empty() {
            return false;
        }
        match tab {
            PackageTab::Translation if self.settings.prepared_translation == expected => {
                self.settings.prepared_translation.clear();
                if self.selected.as_ref().is_some_and(|package| {
                    package.directory.join("bundle.json").display().to_string() == expected
                }) {
                    self.selected = None;
                }
            }
            PackageTab::Ime if self.settings.prepared_ime == expected => {
                self.settings.prepared_ime.clear();
                if self.ime_selected.as_ref().is_some_and(|package| {
                    package
                        .directory
                        .join("ime-package.json")
                        .display()
                        .to_string()
                        == expected
                }) {
                    self.ime_selected = None;
                }
            }
            PackageTab::Performance if self.settings.prepared_performance == expected => {
                self.settings.prepared_performance.clear();
                if self.performance_selected.as_ref().is_some_and(|package| {
                    performance::manifest_path(package).display().to_string() == expected
                }) {
                    self.performance_selected = None;
                }
            }
            _ => return false,
        }
        true
    }
    fn clear_downloads(&mut self, ctx: &egui::Context) {
        // CPU previews and interaction fixtures never perform cache maintenance.
        if self.preview {
            return;
        }
        let directory = engine::data_dir();
        self.work(ctx, move |_| {
            let result = std::panic::catch_unwind(|| cache::clear_downloads(&directory))
                .unwrap_or_else(|_| Err("Clearing downloads was interrupted".into()));
            // Backend failures still complete this work item so receive() can
            // invalidate every prepared selection after an attempted clear.
            Ok(Work::DownloadsCleared(result))
        });
    }
    fn clear_downloads_button(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        busy: bool,
    ) -> egui::Response {
        let response = ui.add_enabled(
            !busy && self.receiver.is_none(),
            egui::Button::new(self.settings.ui_language.text("Clear downloads")).small(),
        );
        if response.clicked() {
            self.clear_downloads(ctx);
        }
        response
    }
    fn tab_source(&self, tab: PackageTab) -> (String, String, String) {
        if tab == PackageTab::Mods {
            return (
                sources::LAUNCHER_REPOSITORY.into(),
                sources::LAUNCHER_RELEASE_API.into(),
                "Local mods".into(),
            );
        }
        if tab == PackageTab::Performance {
            let version = self
                .performance_selected
                .as_ref()
                .map(|p| p.package.version.as_str())
                .or_else(|| {
                    self.performance_release
                        .as_ref()
                        .map(|r| r.version.as_str())
                });
            return (
                performance::REPOSITORY.into(),
                version
                    .map(|v| {
                        format!(
                            "https://github.com/{}/releases/tag/v{v}",
                            performance::REPOSITORY
                        )
                    })
                    .unwrap_or_else(|| performance::RELEASE_URL.into()),
                version
                    .map(|v| format!("Release · v{v}"))
                    .unwrap_or_else(|| "GitHub releases".into()),
            );
        }
        let status = if self.catalog_connected {
            let (count, kind) = match tab {
                PackageTab::Translation => (self.releases.len(), "localization"),
                PackageTab::Ime => (self.ime_releases.len(), "IME"),
                PackageTab::Performance | PackageTab::Mods => unreachable!(),
            };
            if count == 0 {
                format!("Connected · no {kind} packages published")
            } else {
                format!(
                    "Connected · {count} {kind} package{}",
                    if count == 1 { "" } else { "s" }
                )
            }
        } else {
            self.catalog_status.clone()
        };
        (self.settings.repository.clone(), self.source_url(), status)
    }
    fn repository_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, busy: bool) {
        if self.tab == PackageTab::Mods {
            if self.error || !self.status_parts.is_empty() {
                egui::CollapsingHeader::new(self.settings.ui_language.text("Diagnostics"))
                    .id_salt("mods-diagnostics")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(self.presented_status(self.settings.ui_language))
                                .wrap()
                                .selectable(true),
                        );
                    });
            }
            return;
        }
        let language = self.settings.ui_language;
        let (repository, source, status) = self.tab_source(self.tab);
        let id = ui.make_persistent_id("bottom-repository-details");
        let mut clicked = false;
        let mut header = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            id,
            self.preview && self.preview_repository_open,
        )
        .show_header(ui, |ui| {
            clicked = ui
                .add(
                    egui::Label::new(language.text("Repository settings"))
                        .sense(egui::Sense::click()),
                )
                .clicked();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.clear_downloads_button(ui, ctx, busy);
            });
        });
        if clicked {
            header.toggle();
        }
        header.body(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.hyperlink_to(&repository, sources::repository_url(&repository));
                ui.label(RichText::new(language.text(&status)).size(12.).color(MUTED));
                if self.tab == PackageTab::Performance {
                    ui.hyperlink_to(language.text("Release"), &source);
                } else if ui
                    .add_enabled(!busy, egui::Button::new(language.text("Refresh")))
                    .clicked()
                {
                    self.check_updates(ctx);
                }
            });
            if self.tab == PackageTab::Performance {
                ui.hyperlink_to(
                    language.text("Manifest"),
                    self.performance_selected
                        .as_ref()
                        .map(|p| p.package.version.as_str())
                        .or_else(|| {
                            self.performance_release
                                .as_ref()
                                .map(|r| r.version.as_str())
                        })
                        .map(|v| sources::performance_asset_url(v, "manifest.json"))
                        .unwrap_or_else(|| {
                            format!(
                                "https://github.com/{}/releases/latest/download/manifest.json",
                                performance::REPOSITORY
                            )
                        }),
                );
            } else {
                ui.add_enabled_ui(!busy, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(language.text("GitHub repository"));
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.settings.repository)
                                .hint_text(REPOSITORY),
                        );
                        if response.changed() {
                            self.settings.catalog_url = catalog_url(&self.settings.repository);
                            self.invalidate_catalog();
                            self.persist();
                        }
                        if response.lost_focus() {
                            self.check_updates(ctx);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(language.text("Catalog URL"));
                        let response = ui.text_edit_singleline(&mut self.settings.catalog_url);
                        if response.changed() {
                            self.invalidate_catalog();
                            self.persist();
                        }
                        if response.lost_focus() {
                            self.check_updates(ctx);
                        }
                    });
                    ui.hyperlink_to(language.text("Catalog"), source);
                });
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(language.text("Local path"))
                        .size(12.)
                        .color(MUTED),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(engine::data_dir().display().to_string())
                            .size(12.)
                            .color(MUTED),
                    )
                    .wrap()
                    .selectable(true),
                );
                if let Some(log) = &self.launch_log
                    && ui
                        .small_button(language.text("Copy log path"))
                        .on_hover_text(log.display().to_string())
                        .clicked()
                {
                    ui.ctx().copy_text(log.display().to_string());
                }
            });
            if self.error
                || self.update_notice.is_some()
                || !self.status_parts.is_empty()
                || matches!(self.launcher_update, LauncherUpdate::Failed(_))
            {
                egui::CollapsingHeader::new(language.text("Diagnostics"))
                    .id_salt("backend-diagnostics")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(self.presented_status(language))
                                .wrap()
                                .selectable(true),
                        );
                        if let LauncherUpdate::Failed(error) = &self.launcher_update {
                            ui.add(egui::Label::new(error).wrap().selectable(true));
                        }
                        if let Some(notice) = &self.update_notice {
                            ui.add(egui::Label::new(notice).wrap().selectable(true));
                        }
                    });
            }
        });
    }
    fn korean_ime_available(&self) -> bool {
        self.language.eq_ignore_ascii_case("KOR")
    }
    fn game_language_choices(&self) -> Vec<String> {
        let mut languages = vec!["INT".to_owned(), "KOR".to_owned()];
        if !self.settings.launch_language.is_empty()
            && !languages.contains(&self.settings.launch_language)
        {
            languages.push(self.settings.launch_language.clone());
        }
        languages
    }
    fn game_language_caption(&self, code: &str) -> String {
        let language = self.settings.ui_language;
        match code {
            "INT" => format!("{} (INT)", language.text("English")),
            "KOR" => format!("{} (KOR)", language.text("Korean")),
            _ => code.into(),
        }
    }
    fn reconcile_navigation(&mut self) {
        if self.tab == PackageTab::Ime && !self.korean_ime_available() {
            self.tab = PackageTab::Translation;
        }
    }
    fn presented_status(&self, language: UiLanguage) -> String {
        if self.status_parts.is_empty() {
            language.text(&self.status)
        } else {
            self.status_parts
                .iter()
                .map(|part| language.text(part))
                .collect::<Vec<_>>()
                .join("; ")
        }
    }
    // The default footer is a product surface, never a backend diagnostic sink.
    // Every branch returns fixed, short copy; full payloads require Diagnostics.
    fn concise_status(&self) -> &'static str {
        if self.error {
            if self.status == launch::LOCALIZATION_LANGUAGE_MISMATCH {
                return launch::LOCALIZATION_LANGUAGE_MISMATCH;
            }
            return "Operation failed";
        }
        if self.receiver.is_some() {
            return "Working…";
        }
        match self.status.as_str() {
            "Ready" => "Ready",
            "Preparing…" => "Preparing…",
            "Game started" => "Game started",
            "Launch requested" => "Launch requested",
            "Catalog refreshed" => "Refreshed",
            "Mods refreshed" => "Refreshed",
            "Mod added" => "Mod added",
            "Mod enabled" => "Mod enabled",
            "Mod disabled" => "Mod disabled",
            "Mod removed" => "Mod removed",
            "Mods recovered" => "Recovered",
            "Downloads cleared" => "Downloads cleared",
            "Package downloaded/imported and verified. Ready to apply."
            | "IME package verified. Ready to apply."
            | "Performance package ready" => "Package ready",
            _ => "Done",
        }
    }
    fn render(&mut self, root_ui: &mut egui::Ui) {
        let ctx = &root_ui.ctx().clone();
        self.receive();
        self.receive_launcher_update(ctx);
        self.reconcile_navigation();
        let busy = self.receiver.is_some()
            || matches!(
                self.launcher_update,
                LauncherUpdate::Downloading(_) | LauncherUpdate::Restarting
            );
        egui::Panel::top("brand")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(22, 22, 24))
                    .inner_margin(egui::Margin::symmetric(24, 18)),
            )
            .show(root_ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new(&self.logo).fit_to_exact_size(egui::vec2(28., 28.)));
                    ui.label(
                        RichText::new(self.settings.ui_language.text("PaladinsCat Launcher"))
                            .font(bold_font(20.)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(language) = language_menu::show(ui, self.settings.ui_language) {
                            self.settings.ui_language = language;
                            self.persist();
                            ctx.request_repaint();
                        }
                        self.launcher_update_control(ui, ctx, busy);
                    });
                });
            });
        let language = self.settings.ui_language;
        egui::Panel::bottom("footer")
            .frame(egui::Frame::new().fill(BG).inner_margin(12))
            .show(root_ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if busy {
                        ui.spinner();
                    }
                    ui.label(
                        RichText::new(language.text(self.concise_status()))
                            .color(if self.error {
                                Color32::from_rgb(240, 144, 144)
                            } else {
                                MUTED
                            })
                            .size(12.),
                    );
                });
            });
        if self.tab != PackageTab::Mods || self.error || !self.status_parts.is_empty() {
            egui::Panel::bottom("repository-details")
                .frame(egui::Frame::new().fill(BG).inner_margin(12))
                .show(root_ui, |ui| self.repository_panel(ui, ctx, busy));
        }
        egui::CentralPanel::default().show(root_ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                card().show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(
                        RichText::new(language.text("Game installation")).font(bold_font(16.)),
                    );
                    ui.add_enabled_ui(!busy, |ui| {
                        ui.spacing_mut().interact_size.y = 36.;
                        ui.horizontal(|ui| {
                            let has_detected = !self.discovered_games.is_empty();
                            let buttons_width = 200. + if has_detected { 106. } else { 0. };
                            let gaps = if has_detected { 3. } else { 2. };
                            let response = ui.add_sized(
                                [
                                    ui.available_width()
                                        - buttons_width
                                        - gaps * ui.spacing().item_spacing.x,
                                    36.,
                                ],
                                egui::TextEdit::singleline(&mut self.settings.game_folder),
                            );
                            if response.lost_focus() {
                                self.persist();
                                self.refresh_installed();
                            }
                            let detecting = self.discovery.is_some();
                            if ui
                                .add_enabled_ui(!detecting, |ui| {
                                    ui.add_sized(
                                        [100., 36.],
                                        egui::Button::new(language.text(if detecting {
                                            "Detecting…"
                                        } else {
                                            "Detect"
                                        })),
                                    )
                                })
                                .inner
                                .clicked()
                            {
                                self.discover_games(ctx);
                            }
                            if has_detected {
                                let mut chosen = None;
                                egui::ComboBox::from_id_salt("detected-games")
                                    .width(106.)
                                    .selected_text(language.text("Detected"))
                                    .show_ui(ui, |ui| {
                                        for installation in &self.discovered_games {
                                            let path = installation.path.display().to_string();
                                            if ui
                                                .selectable_label(
                                                    self.settings.game_folder == path,
                                                    format!(
                                                        "{} · {path}",
                                                        installation.store.label()
                                                    ),
                                                )
                                                .clicked()
                                            {
                                                chosen = Some(path);
                                            }
                                        }
                                    });
                                if let Some(path) = chosen {
                                    self.settings.game_folder = path;
                                    self.persist();
                                    self.refresh_installed();
                                }
                            }
                            if ui
                                .add_sized([100., 36.], egui::Button::new(language.text("Browse…")))
                                .clicked()
                                && let Some(path) = rfd::FileDialog::new()
                                    .set_title(language.text("Game installation"))
                                    .pick_folder()
                            {
                                self.settings.game_folder = path.display().to_string();
                                self.persist();
                                self.refresh_installed();
                            }
                        });
                    });
                    ui.label(
                        RichText::new(language.text(&format!(
                            "Installed: {}",
                            engine::installed_package_caption(&self.installed)
                        )))
                        .size(12.)
                        .color(MUTED),
                    );
                });
                ui.add_space(12.);
                card().show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.add_enabled_ui(!busy, |ui| {
                        ui.spacing_mut().interact_size.y = 38.;
                        ui.horizontal(|ui| {
                            ui.label(language.text("Game language"));
                            let choices = self.game_language_choices();
                            let mut changed = false;
                            egui::ComboBox::from_id_salt("game-language")
                                .width(120.)
                                .selected_text(
                                    self.game_language_caption(&self.settings.launch_language),
                                )
                                .show_ui(ui, |ui| {
                                    for code in choices {
                                        let caption = self.game_language_caption(&code);
                                        changed |= ui
                                            .selectable_value(
                                                &mut self.settings.launch_language,
                                                code,
                                                caption,
                                            )
                                            .changed();
                                    }
                                });
                            if changed {
                                self.persist();
                            }
                            ui.label(language.text("Profile"));
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.settings.homedir)
                                    .desired_width(150.),
                            );
                            if response.changed() {
                                self.persist();
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_enabled(
                                            !self.settings.game_folder.is_empty()
                                                && (!self.settings.performance_enabled
                                                    || self.performance_selected.is_some()),
                                            egui::Button::new(
                                                RichText::new(language.text("Play")).color(BG),
                                            )
                                            .fill(ACCENT),
                                        )
                                        .clicked()
                                    {
                                        self.play(ctx);
                                    }
                                    let mut platform_changed = false;
                                    egui::ComboBox::from_id_salt("launch-platform")
                                        .width(128.)
                                        .selected_text(self.settings.launch_platform.caption())
                                        .show_ui(ui, |ui| {
                                            for platform in launch::LaunchPlatform::ALL {
                                                platform_changed |= ui
                                                    .selectable_value(
                                                        &mut self.settings.launch_platform,
                                                        platform,
                                                        platform.caption(),
                                                    )
                                                    .changed();
                                            }
                                        })
                                        .response
                                        .on_hover_text(language.text("Launch platform"));
                                    if platform_changed {
                                        self.persist();
                                    }
                                },
                            );
                        });
                    });
                });
                ui.add_space(12.);
                ui.scope(|ui| {
                    // Reserve the complete dropdown height before painting the
                    // row so labels and tabs share its vertical center.
                    ui.spacing_mut().interact_size.y = 38.;
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut self.tab,
                            PackageTab::Performance,
                            language.text("Performance"),
                        );
                        if ui
                            .selectable_label(
                                matches!(self.tab, PackageTab::Translation | PackageTab::Ime),
                                language.text("Localization"),
                            )
                            .clicked()
                            && !matches!(self.tab, PackageTab::Translation | PackageTab::Ime)
                        {
                            self.tab = PackageTab::Translation;
                        }
                        ui.selectable_value(&mut self.tab, PackageTab::Mods, language.text("Mods"));
                        if matches!(self.tab, PackageTab::Translation | PackageTab::Ime) {
                            ui.add_space(12.);
                            ui.add_enabled_ui(!busy, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(language.text("Language"));
                                    let mut languages: Vec<_> =
                                        self.releases.iter().map(|r| r.locale.clone()).collect();
                                    languages.push(self.language.clone());
                                    languages.sort();
                                    languages.dedup();
                                    egui::ComboBox::from_id_salt("language")
                                        .selected_text(&self.language)
                                        .show_ui(ui, |ui| {
                                            for language in languages {
                                                if ui
                                                    .selectable_value(
                                                        &mut self.language,
                                                        language.clone(),
                                                        language,
                                                    )
                                                    .changed()
                                                {
                                                    self.release_index = self
                                                        .releases
                                                        .iter()
                                                        .position(|r| r.locale == self.language)
                                                        .unwrap_or(0);
                                                }
                                            }
                                        });
                                });
                            });
                        }
                    });
                });
                if matches!(self.tab, PackageTab::Translation | PackageTab::Ime) {
                    self.reconcile_navigation();
                    if self.korean_ime_available() {
                        ui.horizontal(|ui| {
                            ui.add_space(12.);
                            ui.selectable_value(
                                &mut self.tab,
                                PackageTab::Translation,
                                language.text("Translation"),
                            );
                            ui.selectable_value(
                                &mut self.tab,
                                PackageTab::Ime,
                                language.text("Korean IME patch"),
                            );
                        });
                    }
                }
                ui.add_space(12.);
                if self.tab == PackageTab::Mods {
                    self.mods_panel(ui, ctx, busy);
                }
                if self.tab == PackageTab::Translation {
                    card().show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(
                            RichText::new(language.text("Translation package"))
                                .font(bold_font(16.)),
                        );
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.horizontal(|ui| {
                                if ui.button(language.text("Import package…")).clicked()
                                    && let Some(path) = rfd::FileDialog::new()
                                        .set_title(language.text("Import package…"))
                                        .add_filter(language.text("Package manifest"), &["json"])
                                        .pick_file()
                                {
                                    let cache = engine::data_dir();
                                    self.work(ctx, move |p| {
                                        engine::import(&path, &cache, p)
                                            .map(|package| Work::Package(Box::new(package)))
                                    });
                                }
                            });
                            egui::ComboBox::from_id_salt("version")
                                .width(ui.available_width())
                                .selected_text(
                                    self.releases
                                        .get(self.release_index)
                                        .filter(|r| r.locale == self.language)
                                        .map(|r| {
                                            engine::package_caption(
                                                &r.bundle_id,
                                                &r.locale,
                                                &r.game_build,
                                            )
                                        })
                                        .or_else(|| {
                                            self.selected.as_ref().map(|p| {
                                                language.text(&format!(
                                                    "Local · {}",
                                                    engine::package_caption(
                                                        &p.package.bundle_id,
                                                        &p.package.locale,
                                                        &p.package.game_build
                                                    )
                                                ))
                                            })
                                        })
                                        .unwrap_or_else(|| {
                                            language.text("No published package selected")
                                        }),
                                )
                                .show_ui(ui, |ui| {
                                    for (i, release) in self
                                        .releases
                                        .iter()
                                        .enumerate()
                                        .filter(|(_, r)| r.locale == self.language)
                                    {
                                        ui.selectable_value(
                                            &mut self.release_index,
                                            i,
                                            engine::package_caption(
                                                &release.bundle_id,
                                                &release.locale,
                                                &release.game_build,
                                            ),
                                        );
                                    }
                                });
                            ui.horizontal(|ui| {
                                let release = self
                                    .releases
                                    .get(self.release_index)
                                    .filter(|r| r.locale == self.language)
                                    .cloned();
                                if ui
                                    .add_enabled(
                                        release.is_some(),
                                        egui::Button::new(language.text("Download package")),
                                    )
                                    .clicked()
                                    && let Some(release) = release
                                {
                                    let cache = engine::data_dir();
                                    self.work(ctx, move |p| {
                                        engine::download(&release, &cache, p)
                                            .map(|package| Work::Package(Box::new(package)))
                                    });
                                }
                                if let Some(selected) = &self.selected {
                                    ui.label(
                                        RichText::new(language.text(&format!(
                                            "Prepared: {}",
                                            engine::package_caption(
                                                &selected.package.bundle_id,
                                                &selected.package.locale,
                                                &selected.package.game_build
                                            )
                                        )))
                                        .color(ACCENT)
                                        .size(12.),
                                    );
                                }
                            });
                        });
                    });
                    ui.add_space(12.);
                    card().show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(
                            RichText::new(language.text("Apply & recovery")).font(bold_font(16.)),
                        );
                        ui.add_enabled_ui(!busy && !self.settings.game_folder.is_empty(), |ui| {
                            ui.horizontal_wrapped(|ui| {
                                let ready = self
                                    .selected
                                    .as_ref()
                                    .is_some_and(|p| p.package.locale == self.language);
                                if ui
                                    .add_enabled(
                                        ready,
                                        egui::Button::new(
                                            RichText::new(language.text("Apply / Update"))
                                                .color(BG),
                                        )
                                        .fill(ACCENT),
                                    )
                                    .clicked()
                                {
                                    self.action(ctx, Action::Apply);
                                }
                                if ui.button(language.text("Previous version")).clicked() {
                                    self.action(ctx, Action::Rollback);
                                }
                                if ui.button(language.text("Restore originals")).clicked() {
                                    self.action(ctx, Action::Restore);
                                }
                                if engine::recovery_pending(&PathBuf::from(
                                    &self.settings.game_folder,
                                )) && ui
                                    .button(language.text("Recover interrupted operation"))
                                    .clicked()
                                {
                                    self.action(ctx, Action::Recover);
                                }
                            });
                        });
                    });
                }
                ui.add_space(12.);
                if self.tab == PackageTab::Ime && self.korean_ime_available() {
                    card().show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(
                            RichText::new(language.text("Korean IME compatibility"))
                                .font(bold_font(16.)),
                        );
                        ui.label(
                            RichText::new(
                                language.text(&format!(
                                    "Configuration: {}",
                                    self.ime_installed.label()
                                )),
                            )
                            .size(12.)
                            .color(MUTED),
                        );
                        ui.add_enabled_ui(!busy, |ui| {
                            egui::ComboBox::from_id_salt("ime-version")
                                .width(ui.available_width())
                                .selected_text(
                                    self.ime_releases
                                        .get(self.ime_index)
                                        .map(|r| format!("{} · {}", r.version, r.game_build))
                                        .or_else(|| {
                                            self.ime_selected.as_ref().map(|c| {
                                                language.text(&format!(
                                                    "Local{} · {}",
                                                    if c.package.draft { " draft" } else { "" },
                                                    c.package.version
                                                ))
                                            })
                                        })
                                        .unwrap_or_else(|| {
                                            language.text("No published IME package selected")
                                        }),
                                )
                                .show_ui(ui, |ui| {
                                    for (i, r) in self.ime_releases.iter().enumerate() {
                                        ui.selectable_value(
                                            &mut self.ime_index,
                                            i,
                                            format!("{} · {}", r.version, r.game_build),
                                        );
                                    }
                                });
                            ui.horizontal_wrapped(|ui| {
                                let release = self.ime_releases.get(self.ime_index).cloned();
                                if ui
                                    .add_enabled(
                                        release.is_some(),
                                        egui::Button::new(language.text("Download IME package")),
                                    )
                                    .clicked()
                                    && let Some(release) = release
                                {
                                    let cache = engine::data_dir();
                                    self.work(ctx, move |p| {
                                        ime::download(&release, &cache, p)
                                            .map(|c| Work::ImePackage(Box::new(c)))
                                    });
                                }
                                if ui.button(language.text("Import IME package…")).clicked()
                                    && let Some(path) = rfd::FileDialog::new()
                                        .set_title(language.text("Import IME package…"))
                                        .add_filter(
                                            language.text("IME package manifest"),
                                            &["json"],
                                        )
                                        .pick_file()
                                {
                                    let cache = engine::data_dir();
                                    self.work(ctx, move |_| {
                                        ime::import(&path, &cache)
                                            .map(|c| Work::ImePackage(Box::new(c)))
                                    });
                                }
                            });
                            if let Some(c) = &self.ime_selected {
                                ui.label(
                                    RichText::new(language.text(&format!(
                                        "Prepared: {}{}",
                                        c.package.version,
                                        if c.package.draft {
                                            " · local draft"
                                        } else {
                                            ""
                                        }
                                    )))
                                    .size(12.)
                                    .color(ACCENT),
                                );
                            }
                            ui.add_enabled_ui(!self.settings.game_folder.is_empty(), |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    let recovery = !self.preview
                                        && ime::recovery_pending(&PathBuf::from(
                                            &self.settings.game_folder,
                                        ));
                                    let prepared = self.ime_selected.is_some();
                                    let mut enabled = self.ime_installed.enabled();
                                    let response = ui.add_enabled(
                                        self.ime_installed.can_toggle(prepared, recovery),
                                        egui::Checkbox::new(
                                            &mut enabled,
                                            language.text("Enable Korean IME fix"),
                                        ),
                                    );
                                    if response.changed()
                                        && let Some(action) = self
                                            .ime_installed
                                            .toggle_action(enabled, prepared, recovery)
                                    {
                                        self.ime_action(ctx, action);
                                    }
                                    if self
                                        .ime_installed
                                        .update_available(self.ime_selected.as_ref())
                                        && ui
                                            .add_enabled(
                                                !recovery,
                                                egui::Button::new(
                                                    language.text("Update IME package"),
                                                ),
                                            )
                                            .clicked()
                                    {
                                        self.ime_action(ctx, ime::Action::Apply);
                                    }
                                    if recovery
                                        && ui
                                            .button(language.text("Recover IME operation"))
                                            .clicked()
                                    {
                                        self.ime_action(ctx, ime::Action::Recover);
                                    }
                                });
                            });
                        });
                    });
                }
                if self.tab == PackageTab::Performance {
                    card().show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(
                            RichText::new(language.text("Performance mod")).font(bold_font(16.)),
                        );
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .button(language.text("Download performance package"))
                                    .clicked()
                                {
                                    let cache = engine::data_dir();
                                    self.work(ctx, move |p| {
                                        performance::download(&cache, p)
                                            .map(|c| Work::PerformancePackage(Box::new(c)))
                                    });
                                }
                                if ui
                                    .button(language.text("Import performance package…"))
                                    .clicked()
                                    && let Some(path) = rfd::FileDialog::new()
                                        .set_title(language.text("Import performance package…"))
                                        .add_filter(
                                            language.text("Performance manifest"),
                                            &["json"],
                                        )
                                        .pick_file()
                                {
                                    let cache = engine::data_dir();
                                    self.work(ctx, move |_| {
                                        performance::import(&path, &cache)
                                            .map(|c| Work::PerformancePackage(Box::new(c)))
                                    });
                                }
                            });
                            if let Some(package) = &self.performance_selected {
                                ui.label(
                                    RichText::new(
                                        language.text(&format!(
                                            "Prepared: v{}",
                                            package.package.version
                                        )),
                                    )
                                    .color(ACCENT)
                                    .size(12.),
                                );
                            }
                            if ui
                                .add_enabled(
                                    self.performance_selected.is_some()
                                        || self.settings.performance_enabled,
                                    egui::Checkbox::new(
                                        &mut self.settings.performance_enabled,
                                        language.text("Enable performance mod on next launch"),
                                    ),
                                )
                                .changed()
                            {
                                self.persist();
                            }
                            if ui
                                .add_enabled(
                                    self.settings.performance_enabled,
                                    egui::Checkbox::new(
                                        &mut self.settings.rollback_input,
                                        language.text("Use previous input version"),
                                    ),
                                )
                                .changed()
                            {
                                self.persist();
                            }
                        });
                    });
                }
                ui.add_space(12.);
            });
        });
    }
}
impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.render(ui);
        if self.update_frames == 0 {
            self.update_frames = 1;
            ui.ctx().request_repaint();
        } else if self.update_frames == 1 {
            // The first frame has returned through eframe's rendering loop.
            // Acknowledge only on the next UI callback, after renderer checks.
            self.update_frames = 2;
            let result = self_update::mark_ready();
            if let Err(error) = result {
                self.status_parts
                    .push(format!("Launcher update readiness: {error}"));
            }
            let (sender, receiver) = mpsc::channel();
            self.update_receiver = Some(receiver);
            self.launcher_update = LauncherUpdate::Checking;
            let repaint = ui.ctx().clone();
            thread::spawn(move || {
                let _ = sender.send(UpdateEvent::Cleanup(self_update::startup_cleanup()));
                repaint.request_repaint();
            });
        }
    }
}
fn main() -> eframe::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if let Some(result) = self_update::helper_mode(&args) {
        return result.map_err(|e| eframe::Error::AppCreation(std::io::Error::other(e).into()));
    }
    if args.len() == 2 && matches!(args[1].as_str(), "--help" | "-h") {
        println!(
            "PaladinsCat Launcher\n  --preview|--preview-ime|--preview-performance OUTPUT.png\n  --preview-kor|--preview-ime-kor|--preview-performance-kor OUTPUT.png\n  --preview-language-menu|--preview-language-menu-kor OUTPUT.png\n  --inspect-startup OUTPUT.json\n  --vm-renderer-proof PROOF.json (VM-only; requires a fresh RTX 5080 host proof)\nUse paladinscat-launcher-cli for explicit download, --dry-run and --launch operations."
        );
        return Ok(());
    }
    if args.len() == 3 && args[1] == "--inspect-startup" {
        let settings = Settings::load();
        let startup = Startup::load(&settings, &engine::data_dir(), &|_| {});
        let report = serde_json::json!({
            "repository": settings.repository,
            "catalog_url": settings.source_url(),
            "catalog": startup.catalog.as_ref().map(|c|serde_json::json!({"translations":c.packages.len(),"ime_packages":c.ime_packages.len()})),
            "translation": startup.translation.as_ref().map(|p|p.as_ref().map(|p|p.package.bundle_id.clone())),
            "ime": startup.ime.as_ref().map(|p|p.as_ref().map(|p|serde_json::json!({"version":p.package.version,"draft":p.package.draft,"dll_sha256":p.package.dll_sha256}))),
            "performance": startup.performance.as_ref().map(|p|p.as_ref().map(|p|p.package.version.clone())),
            "feature_sources": {
                "localization": {"repository": settings.repository, "url": settings.source_url(), "packages": startup.catalog.as_ref().map(|c|c.packages.len())},
                "ime": {"repository": settings.repository, "url": settings.source_url(), "packages": startup.catalog.as_ref().map(|c|c.ime_packages.len())},
                "performance": {"repository": performance::REPOSITORY, "url": performance::RELEASE_URL, "version": startup.performance_release.as_ref().map(|r| &r.version)}
            },
            "cpu_only": true,
            "game_files_changed": false,
        });
        engine::save_settings(&PathBuf::from(&args[2]), &report)
            .map_err(|e| eframe::Error::AppCreation(std::io::Error::other(e).into()))?;
        return Ok(());
    }
    if args.len() == 3
        && matches!(
            args[1].as_str(),
            "--preview"
                | "--preview-ime"
                | "--preview-performance"
                | "--preview-prepared"
                | "--preview-prepared-kor"
                | "--preview-ime-prepared"
                | "--preview-performance-prepared"
                | "--preview-kor"
                | "--preview-ime-kor"
                | "--preview-performance-kor"
                | "--preview-language-menu"
                | "--preview-language-menu-kor"
                | "--preview-details"
                | "--preview-details-kor"
                | "--preview-update"
                | "--preview-update-kor"
                | "--preview-mods"
                | "--preview-mods-kor"
        )
    {
        preview::render(
            &PathBuf::from(&args[2]),
            if args[1].contains("-performance")
                || args[1].contains("-language-menu")
                || args[1].contains("-details")
                || args[1].contains("-update")
            {
                PackageTab::Performance
            } else if args[1].contains("-mods") {
                PackageTab::Mods
            } else if args[1].contains("-ime") {
                PackageTab::Ime
            } else {
                PackageTab::Translation
            },
            args[1].contains("-prepared"),
            args[1].ends_with("-kor").then_some(UiLanguage::Korean),
            args[1].contains("-language-menu"),
            args[1].contains("-details"),
            args[1].contains("-update"),
        )
        .expect("Headless GUI preview");
        return Ok(());
    }
    let vm_proof_path = if args.len() == 3 && args[1] == "--vm-renderer-proof" {
        Some(PathBuf::from(&args[2]))
    } else if args.len() == 1 {
        None
    } else {
        return Err(eframe::Error::AppCreation(
            std::io::Error::other("Unknown launcher arguments; use --help").into(),
        ));
    };
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/paladinscat.png"))
        .map_err(|e| eframe::Error::AppCreation(e.into()))?;

    if let Some(proof_path) = vm_proof_path {
        let selected = vm_renderer::VmRenderer::preflight(&proof_path)
            .map_err(|e| eframe::Error::AppCreation(std::io::Error::other(e).into()))?;
        let native_options = eframe::NativeOptions {
            renderer: eframe::Renderer::Glow,
            viewport: egui::ViewportBuilder::default()
                .with_icon(icon)
                .with_inner_size(DEFAULT_WINDOW_SIZE)
                .with_min_inner_size([720., 640.]),
            ..Default::default()
        };
        return eframe::run_native(
            "PaladinsCat Launcher",
            native_options,
            Box::new(move |cc| {
                selected.verify_context(cc).map_err(std::io::Error::other)?;
                Ok(Box::new(App::new(&cc.egui_ctx, false)))
            }),
        );
    }

    let (wgpu_options, selected) = graphics_device::configuration()
        .map_err(|e| eframe::Error::AppCreation(std::io::Error::other(e).into()))?;
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        viewport: egui::ViewportBuilder::default()
            .with_icon(icon)
            .with_inner_size(DEFAULT_WINDOW_SIZE)
            .with_min_inner_size([720., 640.]),
        ..Default::default()
    };
    eframe::run_native(
        "PaladinsCat Launcher",
        native_options,
        Box::new(move |cc| {
            graphics_device::verify_renderer(cc, &selected).map_err(std::io::Error::other)?;
            Ok(Box::new(App::new(&cc.egui_ctx, false)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mods_page_has_localized_import_and_actual_registry_rows_without_other_package_controls() {
        for language in [UiLanguage::English, UiLanguage::Korean] {
            let ctx = egui::Context::default();
            let mut app = App::new(&ctx, true);
            app.settings.ui_language = language;
            app.tab = PackageTab::Mods;
            app.settings.game_folder = "C:\\Owned fixture\\Paladins".into();
            app.mods_folder = app.settings.game_folder.clone();
            app.mods = Ok(vec![mods::ModInfo {
                id: "fixture".into(),
                name: "Fixture mod".into(),
                version: "1.2.3".into(),
                author: "Fixture author".into(),
                kind: "V2".into(),
                enabled: true,
            }]);
            let settings = serde_json::to_value(&app.settings).unwrap();
            let text = rendered_text(&mut app, &ctx);
            for label in [
                "Tempest mods",
                "Add mod…",
                "Name",
                "Version",
                "Author",
                "Status",
                "Enabled",
                "Disable",
                "Remove",
                "Actions",
            ] {
                assert!(
                    text.contains(&language.text(label)),
                    "Missing {label}: {text}"
                );
            }
            assert!(text.contains("Fixture mod") && text.contains("1.2.3"));
            assert!(!text.contains(&language.text("Translation package")));
            assert!(!text.contains(&language.text("Performance mod")));
            assert!(!text.contains(&language.text("Repository settings")));
            assert!(!app.allow_native_mods && !app.replace_mod_files);
            assert_eq!(serde_json::to_value(&app.settings).unwrap(), settings);
            fn visible(shape: &egui::Shape, clip: egui::Rect, needle: &str) -> Option<bool> {
                match shape {
                    egui::Shape::Text(text) if text.galley.text() == needle => Some(
                        clip.contains_rect(egui::Rect::from_min_size(text.pos, text.galley.size())),
                    ),
                    egui::Shape::Vec(shapes) => {
                        shapes.iter().find_map(|shape| visible(shape, clip, needle))
                    }
                    _ => None,
                }
            }
            let mut row_visible = false;
            for _ in 0..3 {
                let frame = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(DEFAULT_WINDOW_SIZE[0], DEFAULT_WINDOW_SIZE[1]),
                        )),
                        ..Default::default()
                    },
                    |ui| app.render(ui),
                );
                row_visible = [
                    "Fixture mod".to_owned(),
                    language.text("Disable"),
                    language.text("Remove"),
                ]
                .iter()
                .all(|needle| {
                    frame
                        .shapes
                        .iter()
                        .find_map(|shape| visible(&shape.shape, shape.clip_rect, needle))
                        .unwrap_or(false)
                });
            }
            assert!(
                row_visible,
                "The first installed mod must be visible at the default window size"
            );
        }
    }

    #[test]
    fn late_mod_listing_does_not_replace_another_game_selection_and_import_status_stays_short() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        app.settings.game_folder = "C:\\current".into();
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Done(Ok(Work::Mods(
            "C:\\former".into(),
            Vec::new(),
            "Mods refreshed",
        ))))
        .unwrap();
        app.receiver = Some(rx);
        app.receive();
        assert_ne!(app.mods_folder, "C:\\former");
        for status in ["Mod added", "Mod enabled", "Mod disabled", "Mod removed"] {
            app.status = status.into();
            assert_eq!(app.concise_status(), status);
        }
        app.status = "native import path and diagnostic details".into();
        app.error = true;
        assert_eq!(app.concise_status(), "Operation failed");
    }

    #[test]
    fn mod_row_controls_dispatch_actions_and_managed_or_busy_rows_are_inert() {
        for language in [UiLanguage::English, UiLanguage::Korean] {
            for (enabled, label) in [(true, "Disable"), (false, "Enable"), (true, "Remove")] {
                let ctx = egui::Context::default();
                let mut app = App::new(&ctx, true);
                app.settings.ui_language = language;
                app.tab = PackageTab::Mods;
                // Safe worker-dispatch failure; no installed game is touched.
                app.settings.game_folder = "C:\\PaladinsCat-nonexistent-control-fixture".into();
                app.mods_folder = app.settings.game_folder.clone();
                app.mods = Ok(vec![mods::ModInfo {
                    id: "fixture".into(),
                    name: "Fixture".into(),
                    version: "1".into(),
                    author: "Author".into(),
                    kind: "V2".into(),
                    enabled,
                }]);
                let positions = rendered_text_positions(&mut app, &ctx);
                let point = positions
                    .iter()
                    .find(|(text, _)| *text == language.text(label))
                    .expect("row action")
                    .1
                    .center();
                click_at_point(&mut app, &ctx, point);
                let event = app
                    .receiver
                    .as_ref()
                    .expect("Action started shared worker")
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                assert!(matches!(event, Event::Completion(Err(_), _)));
                app.receiver = None;

                app.mods.as_mut().unwrap()[0].id = "paladinscat-ime".into();
                let positions = rendered_text_positions(&mut app, &ctx);
                let point = positions
                    .iter()
                    .find(|(text, _)| *text == language.text(label))
                    .unwrap()
                    .1
                    .center();
                click_at_point(&mut app, &ctx, point);
                assert!(
                    app.receiver.is_none(),
                    "Managed mods use their own controls"
                );

                app.mods.as_mut().unwrap()[0].id = "fixture".into();
                let (sender, receiver) = mpsc::channel();
                app.receiver = Some(receiver);
                let positions = rendered_text_positions(&mut app, &ctx);
                let point = positions
                    .iter()
                    .find(|(text, _)| *text == language.text(label))
                    .unwrap()
                    .1
                    .center();
                click_at_point(&mut app, &ctx, point);
                assert!(app.receiver.as_ref().unwrap().try_recv().is_err());
                drop(sender);
            }
        }
    }

    #[test]
    fn self_update_header_stays_compact_and_never_changes_game_settings() {
        for language in [UiLanguage::English, UiLanguage::Korean] {
            let ctx = egui::Context::default();
            let mut app = App::new(&ctx, true);
            app.settings.ui_language = language;
            let before = serde_json::to_value(&app.settings).unwrap();
            let (sender, receiver) = mpsc::channel();
            sender
                .send(UpdateEvent::Checked(Ok(self_update::Check::Available(
                    self_update::Release {
                        version: "0.3.9".into(),
                        sha256: "a".repeat(64),
                        bytes: 1024,
                    },
                ))))
                .unwrap();
            app.update_receiver = Some(receiver);
            let positions = rendered_text_positions(&mut app, &ctx);
            let update = positions
                .iter()
                .find(|(t, _)| t == &language.text("Update v0.3.9"))
                .unwrap()
                .1;
            let menu = positions
                .iter()
                .find(|(t, _)| t == language.code())
                .unwrap()
                .1;
            let title = positions
                .iter()
                .find(|(t, _)| t == &language.text("PaladinsCat Launcher"))
                .unwrap()
                .1;
            assert!(update.left() > title.right() && update.right() < menu.left());
            assert!((update.center().y - menu.center().y).abs() < 3.);
            assert_eq!(serde_json::to_value(&app.settings).unwrap(), before);
            assert!(!rendered_text(&mut app, &ctx).contains(&"a".repeat(64)));
            let (sender, receiver) = mpsc::channel();
            sender
                .send(UpdateEvent::Armed(Err("owned staging error".into())))
                .unwrap();
            app.update_receiver = Some(receiver);
            app.receive_launcher_update(&ctx);
            assert!(
                matches!(&app.launcher_update,LauncherUpdate::Failed(e) if e=="owned staging error")
            );
            assert!(rendered_text(&mut app, &ctx).contains(&language.text("Retry update")));
            assert!(!rendered_text(&mut app, &ctx).contains("owned staging error"));
        }
    }

    fn discovery_fixture() -> Vec<game_discovery::Installation> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-game-discovery-ui")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
        for dir in ["Binaries/Win64", "Engine", "ChaosGame"] {
            std::fs::create_dir_all(path.join(dir)).unwrap();
        }
        std::fs::write(
            path.join("Binaries/Win64/Paladins.exe"),
            b"fixture, never executed",
        )
        .unwrap();
        vec![game_discovery::Installation {
            path,
            store: game_discovery::Store::Steam,
        }]
    }

    #[test]
    fn discovered_folder_arrives_while_catalog_is_still_pending() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        app.settings.ui_language = UiLanguage::Korean;
        app.settings.homedir = "saved-profile".into();
        app.settings.rollback_input = true;
        let mut expected = serde_json::to_value(&app.settings).unwrap();
        let installations = discovery_fixture();
        expected["game_folder"] = serde_json::json!(installations[0].path.display().to_string());
        let (catalog_sender, catalog_receiver) = mpsc::channel();
        app.receiver = Some(catalog_receiver);
        let (sender, receiver) = mpsc::channel();
        sender
            .send(GameDiscovery {
                requested_folder: String::new(),
                installations,
            })
            .unwrap();
        app.discovery = Some(receiver);
        app.receive();
        assert_eq!(serde_json::to_value(&app.settings).unwrap(), expected);
        assert!(app.receiver.is_some());
        assert!(app.discovery.is_none());
        drop(catalog_sender);
    }

    #[test]
    fn late_discovery_preserves_manual_edits_and_multiple_installs_require_choice() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        app.settings.game_folder = "D:/new-manual-selection".into();
        let expected = serde_json::to_value(&app.settings).unwrap();
        app.accept_discovery(GameDiscovery {
            requested_folder: String::new(),
            installations: discovery_fixture(),
        });
        assert_eq!(serde_json::to_value(&app.settings).unwrap(), expected);
        assert_eq!(app.discovered_games.len(), 1);

        app.settings.game_folder.clear();
        let mut installations = discovery_fixture();
        installations.extend(discovery_fixture());
        app.accept_discovery(GameDiscovery {
            requested_folder: String::new(),
            installations,
        });
        assert!(app.settings.game_folder.is_empty());
        assert_eq!(app.discovered_games.len(), 2);
        let text = rendered_text(&mut app, &ctx);
        assert!(
            text.contains("Detected") && text.contains("Browse…") && text.contains("Detect"),
            "{text}"
        );
        app.settings.ui_language = UiLanguage::Korean;
        let text = rendered_text(&mut app, &ctx);
        assert!(
            text.contains("검색 결과") && text.contains("찾아보기…") && text.contains("자동 검색"),
            "{text}"
        );
    }

    #[test]
    fn legacy_patcher_settings_preserve_selected_packages_without_enabling_new_mods() {
        let settings: Settings = serde_json::from_str(r#"{"game_folder":"C:/Paladins","repository":"PaladinsCat/PaladinsCat-locales","catalog_url":"","prepared_translation":"bundle.json","prepared_ime":"ime-package.json"}"#).unwrap();
        assert_eq!(settings.prepared_translation, "bundle.json");
        assert_eq!(settings.prepared_ime, "ime-package.json");
        assert!(!settings.performance_enabled);
        assert!(!settings.rollback_input);
        assert_eq!(settings.launch_language, "INT");
        assert_eq!(settings.homedir, "PaladinsCat");
        assert_eq!(settings.source_url(), catalog_url(REPOSITORY));

        // A profile change selects a different game home, including its replays.
        // New defaults must not redirect an explicitly saved existing profile.
        let mut saved = serde_json::to_value(&settings).unwrap();
        saved["launch_language"] = serde_json::json!("KOR");
        saved["homedir"] = serde_json::json!("existing-profile");
        let restored: Settings = serde_json::from_value(saved).unwrap();
        assert_eq!(restored.launch_language, "KOR");
        assert_eq!(restored.homedir, "existing-profile");
        assert_eq!(restored.prepared_translation, "bundle.json");
        assert_eq!(restored.prepared_ime, "ime-package.json");
    }

    #[test]
    fn retired_tempest_setting_preserves_existing_launch_and_package_choices() {
        let mut legacy = serde_json::json!({
            "game_folder": "C:/Games/팔라딘/Season 8.1",
            "repository": "example/existing-locales",
            "catalog_url": "https://example.invalid/existing-catalog.json",
            "prepared_translation": "existing-translation/bundle.json",
            "prepared_ime": "existing-ime/ime-package.json",
            "prepared_performance": "existing-performance/performance-package.json",
            "performance_enabled": true,
            "rollback_input": true,
            "launch_language": "KOR",
            "homedir": "existing-profile",
            "tempest_cli": "C:/Former Tempest/tempest-cli.exe"
        });
        let mut expected = legacy.clone();
        expected.as_object_mut().unwrap().remove("tempest_cli");
        expected["ui_language"] = serde_json::json!("ENG");
        expected["launch_platform"] = serde_json::json!("pc");
        // The retired field is ignored by serde, including stale values that
        // would otherwise cause Settings::load to discard the whole file.
        for retired in [
            serde_json::json!("C:/Former Tempest/tempest-cli.exe"),
            serde_json::Value::Null,
            serde_json::json!({ "retired": true }),
        ] {
            legacy["tempest_cli"] = retired;
            let settings: Settings = serde_json::from_value(legacy.clone()).unwrap();
            let saved = serde_json::to_value(&settings).unwrap();
            assert_eq!(saved, expected);
            let reloaded: Settings = serde_json::from_value(saved).unwrap();
            assert_eq!(serde_json::to_value(&reloaded).unwrap(), expected);
        }
    }

    #[test]
    fn disabled_performance_does_not_pass_a_stale_rollback_selection() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        app.settings.rollback_input = true;
        let options = app.launch_options();
        assert!(options.performance.is_none());
        assert!(!options.rollback_input);
    }

    fn ime_package(version: &str) -> ime::Cached {
        ime::Cached {
            package: ime::Package {
                schema_version: 1,
                package_id: format!("paladinscat-ime-{}", version.replace('.', "-")),
                version: version.into(),
                game_build: "8.1".into(),
                platform: "Win64".into(),
                locale: "KOR".into(),
                native_abi: [4, 9, 7, 1],
                game_fingerprint: engine::Fingerprint {
                    path: "Binaries/Win64/Paladins.exe".into(),
                    sha256: "a".repeat(64),
                },
                dll_sha256: "b".repeat(64),
                dll_bytes: 2048,
                source_commit: "c".repeat(40),
                draft: false,
                notes: String::new(),
                download: None,
            },
            directory: PathBuf::new(),
        }
    }

    #[test]
    fn ime_toggle_uses_configuration_instead_of_download_presence() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        app.accept_ime(ime_package("0.1.0"));
        assert!(!app.ime_installed.enabled());
        assert!(matches!(
            app.ime_installed.toggle_action(true, true, false),
            Some(ime::Action::Apply)
        ));
        // A request does not optimistically change the displayed configuration.
        assert!(!app.ime_installed.enabled());
        app.ime_installed =
            ImeInstallState::Enabled(Box::new(app.ime_selected.take().unwrap().package));
        // A verified enabled mod can be disabled even without a prepared download.
        assert!(matches!(
            app.ime_installed.toggle_action(false, false, false),
            Some(ime::Action::Remove)
        ));
        assert!(
            app.ime_installed
                .toggle_action(true, false, false)
                .is_none()
        );
    }

    #[test]
    fn unknown_and_interrupted_ime_configurations_disable_the_toggle() {
        let invalid = ImeInstallState::Invalid("modified DLL".into());
        assert!(!invalid.can_toggle(true, false));
        assert!(invalid.toggle_action(true, true, false).is_none());
        assert!(!ImeInstallState::NoGame.can_toggle(true, false));
        assert!(!ImeInstallState::Disabled.can_toggle(false, false));
        let enabled = ImeInstallState::Enabled(Box::new(ime_package("0.1.0").package));
        assert!(!enabled.can_toggle(true, true));
        assert!(enabled.toggle_action(false, true, true).is_none());
        assert!(
            ImeInstallState::Disabled
                .toggle_action(true, true, true)
                .is_none()
        );
    }

    #[test]
    fn preparing_another_version_does_not_disable_or_implicitly_update_the_mod() {
        let old = ime_package("0.1.0");
        let enabled = ImeInstallState::Enabled(Box::new(old.package.clone()));
        assert!(!enabled.update_available(Some(&old)));
        assert!(enabled.update_available(Some(&ime_package("0.2.0"))));
        assert!(enabled.enabled());
        assert!(enabled.toggle_action(true, true, false).is_none());
        assert!(!ImeInstallState::Disabled.update_available(Some(&old)));
    }

    #[test]
    fn configured_catalog_is_used_with_repository_link_retained() {
        let mut settings = Settings {
            catalog_url: "https://github.com/PaladinsCat/PaladinsCat-locales/releases/download/preview/catalog.json".into(),
            ..Settings::default()
        };
        assert_eq!(settings.source_url(), settings.catalog_url);
        assert_eq!(settings.repository, REPOSITORY);
        settings.catalog_url.clear();
        assert_eq!(settings.source_url(), catalog_url(REPOSITORY));
    }

    #[test]
    fn empty_and_failed_catalogs_have_distinct_visible_statuses() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        let deliver = |app: &mut App, result| {
            let (sender, receiver) = mpsc::channel();
            sender.send(Event::Done(result)).unwrap();
            app.receiver = Some(receiver);
            app.receive();
        };
        deliver(
            &mut app,
            Ok(Work::Catalog(
                engine::parse_catalog(br#"{"schema_version":1,"packages":[],"ime_packages":[]}"#)
                    .unwrap(),
            )),
        );
        assert_eq!(
            app.catalog_status,
            "Connected · no downloadable packages published"
        );
        assert!(!app.error);
        assert_eq!(
            app.tab_source(PackageTab::Translation).2,
            "Connected · no localization packages published"
        );
        assert_eq!(
            app.tab_source(PackageTab::Ime).2,
            "Connected · no IME packages published"
        );
        app.catalog_status = "Connecting to GitHub…".into();
        deliver(&mut app, Err("offline".into()));
        assert_eq!(app.catalog_status, "Catalog unavailable");
        assert_eq!(app.status, "offline");
        assert!(app.error);
        assert_eq!(
            app.tab_source(PackageTab::Translation).2,
            "Catalog unavailable"
        );
        assert_eq!(app.tab_source(PackageTab::Performance).2, "GitHub releases");
    }

    fn catalog_fixture(translation: bool, ime: bool) -> engine::Catalog {
        let mut value = serde_json::json!({"schema_version":1,"packages":[],"ime_packages":[]});
        if translation {
            value["packages"] = serde_json::json!([{"bundle_id":"kor-fixture","locale":"KOR","game_build":"8.1","manifest_url":"https://github.com/PaladinsCat/PaladinsCat-locales/releases/download/fixture/locale-package.json","manifest_sha256":"a".repeat(64)}]);
        }
        if ime {
            value["ime_packages"] = serde_json::json!([{"package_id":"ime-fixture","version":"0.1.0","game_build":"8.1","manifest_url":"https://github.com/PaladinsCat/PaladinsCat-locales/releases/download/fixture/ime-package.json","manifest_sha256":"b".repeat(64)}]);
        }
        engine::parse_catalog(&serde_json::to_vec(&value).unwrap()).unwrap()
    }

    #[test]
    fn performance_source_displays_discovered_and_selected_versions() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        let asset = performance::Asset {
            name: String::new(),
            sha256: String::new(),
            bytes: 0,
        };
        for version in ["0.1.1", "71.23.109"] {
            app.performance_release = Some(Box::new(performance::Release {
                version: version.into(),
                manifest: asset.clone(),
                archive: asset.clone(),
                checksums: asset.clone(),
            }));
            let (_, url, label) = app.tab_source(PackageTab::Performance);
            assert_eq!(label, format!("Release · v{version}"));
            assert!(url.ends_with(&format!("/releases/tag/v{version}")));
        }
    }

    #[test]
    fn feature_counts_are_independent_and_downloads_do_not_hide_releases() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        app.accept_catalog(catalog_fixture(true, false));
        assert_eq!(
            app.tab_source(PackageTab::Translation).2,
            "Connected · 1 localization package"
        );
        assert_eq!(
            app.tab_source(PackageTab::Ime).2,
            "Connected · no IME packages published"
        );
        app.accept_catalog(catalog_fixture(false, true));
        assert_eq!(
            app.tab_source(PackageTab::Translation).2,
            "Connected · no localization packages published"
        );
        assert_eq!(
            app.tab_source(PackageTab::Ime).2,
            "Connected · 1 IME package"
        );
        app.accept_ime(ime_package("0.1.0"));
        assert_eq!(app.ime_releases.len(), 1);
        assert_eq!(
            app.tab_source(PackageTab::Ime).2,
            "Connected · 1 IME package"
        );
    }

    #[test]
    fn changing_catalog_source_invalidates_only_its_published_lists() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        app.accept_catalog(catalog_fixture(true, true));
        app.accept_ime(ime_package("0.1.0"));
        app.settings.repository = "example/custom-locales".into();
        app.settings.catalog_url = catalog_url(&app.settings.repository);
        app.invalidate_catalog();
        assert!(app.releases.is_empty() && app.ime_releases.is_empty());
        assert!(app.ime_selected.is_some());
        let (repository, url, status) = app.tab_source(PackageTab::Ime);
        assert_eq!(repository, "example/custom-locales");
        assert!(url.contains("example/custom-locales/main/"));
        assert_eq!(status, "Catalog not checked");
        assert_eq!(
            app.tab_source(PackageTab::Performance).0,
            performance::REPOSITORY
        );
    }

    #[test]
    fn catalog_completion_after_tab_switch_keeps_the_performance_source() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        let (sender, receiver) = mpsc::channel();
        app.receiver = Some(receiver);
        app.tab = PackageTab::Performance;
        sender
            .send(Event::Done(Ok(Work::Catalog(catalog_fixture(true, true)))))
            .unwrap();
        app.receive();
        assert_eq!(app.tab_source(app.tab).0, performance::REPOSITORY);
        assert_eq!(app.tab_source(app.tab).2, "GitHub releases");
        assert_eq!(app.status, "Catalog refreshed");
        assert_eq!(app.releases.len(), 1);
        assert_eq!(app.ime_releases.len(), 1);
    }

    fn rendered_text(app: &mut App, ctx: &egui::Context) -> String {
        fn text(shape: &egui::epaint::Shape, result: &mut String) {
            match shape {
                egui::epaint::Shape::Text(value) => {
                    result.push_str(value.galley.text());
                    result.push('\n');
                }
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        text(shape, result);
                    }
                }
                _ => {}
            }
        }
        let mut result = String::new();
        for _ in 0..3 {
            result.clear();
            let mut frame = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(880., 960.),
                    )),
                    ..Default::default()
                },
                |ui| app.render(ui),
            );
            for shape in frame.shapes {
                text(&shape.shape, &mut result);
            }
            // This CPU assertion inspects text only, without a texture renderer.
            frame.textures_delta.clear();
        }
        result
    }

    #[test]
    fn korean_ime_is_a_conditional_localization_child_and_preserves_configuration() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        assert!(app.tab == PackageTab::Performance);
        let result = rendered_text(&mut app, &ctx);
        assert!(result.find("Performance").unwrap() < result.find("Localization").unwrap());
        assert!(!result.contains("Korean IME patch"));

        app.accept_catalog(catalog_fixture(true, true));
        app.tab = PackageTab::Translation;
        let result = rendered_text(&mut app, &ctx);
        assert!(result.find("Localization").unwrap() < result.find("Language").unwrap());
        assert!(result.find("Language").unwrap() < result.find("Korean IME patch").unwrap());
        assert!(!result.contains("Korean IME compatibility"));

        app.tab = PackageTab::Ime;
        app.ime_installed = ImeInstallState::Enabled(Box::new(ime_package("0.1.0").package));
        app.accept_ime(ime_package("0.1.0"));
        let saved = serde_json::to_value(&app.settings).unwrap();
        let result = rendered_text(&mut app, &ctx);
        assert!(result.contains("Language\n"));
        assert!(result.contains("Korean IME compatibility"));

        app.language = "ENG".into();
        let result = rendered_text(&mut app, &ctx);
        assert!(app.tab == PackageTab::Translation);
        assert!(!result.contains("Korean IME patch"));
        assert!(!result.contains("Korean IME compatibility"));
        assert!(!result.contains("Enable Korean IME fix"));
        assert!(app.ime_installed.enabled());
        assert!(app.ime_selected.is_some());
        assert_eq!(serde_json::to_value(&app.settings).unwrap(), saved);
    }

    #[test]
    fn repository_details_start_hidden_at_bottom_and_remain_feature_scoped() {
        for tab in [
            PackageTab::Translation,
            PackageTab::Ime,
            PackageTab::Performance,
        ] {
            let ctx = egui::Context::default();
            let mut app = App::new(&ctx, true);
            app.tab = tab;
            app.accept_catalog(catalog_fixture(true, true));
            let result = rendered_text(&mut app, &ctx);
            let repository = if tab == PackageTab::Performance {
                performance::REPOSITORY
            } else {
                REPOSITORY
            };
            assert!(!result.contains(repository), "{result}");
            assert!(
                !result.contains("Connected ·") && !result.contains("Pinned release ·"),
                "{result}"
            );
            let (_, header) = rendered_text_positions(&mut app, &ctx)
                .into_iter()
                .find(|(text, _)| text == "Repository settings")
                .unwrap();
            assert!(header.top() > 850., "Repository header: {header:?}");
            click_at_point(&mut app, &ctx, header.center());
            let result = rendered_text(&mut app, &ctx);
            assert!(result.contains(repository), "{result}");
            if tab == PackageTab::Performance {
                assert!(!result.contains(REPOSITORY), "{result}");
                assert!(!result.contains("no downloadable"), "{result}");
            }
        }
    }

    fn language_menu_frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
        let mut frame = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(880., 960.),
                )),
                events,
                ..Default::default()
            },
            |ui| app.render(ui),
        );
        // Interaction fixtures deliberately omit a renderer.
        frame.textures_delta.clear();
    }

    fn click_language_widget(app: &mut App, ctx: &egui::Context, id: egui::Id) {
        let point = ctx
            .read_response(id)
            .expect("Language control rendered")
            .rect
            .center();
        click_at_point(app, ctx, point);
    }

    fn click_at_point(app: &mut App, ctx: &egui::Context, point: egui::Pos2) {
        language_menu_frame(
            app,
            ctx,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        language_menu_frame(
            app,
            ctx,
            vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        language_menu_frame(app, ctx, Vec::new());
    }

    fn rendered_text_positions(app: &mut App, ctx: &egui::Context) -> Vec<(String, egui::Rect)> {
        fn collect(shape: &egui::Shape, output: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => output.push((
                    text.galley.text().into(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, output);
                    }
                }
                _ => {}
            }
        }
        let mut output = Vec::new();
        for _ in 0..3 {
            let mut frame = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(880., 960.),
                    )),
                    ..Default::default()
                },
                |ui| app.render(ui),
            );
            output.clear();
            for shape in &frame.shapes {
                collect(&shape.shape, &mut output);
            }
            frame.textures_delta.clear();
        }
        output
    }

    #[test]
    fn localization_language_control_shares_the_tab_row_in_both_ui_languages() {
        for language in [UiLanguage::English, UiLanguage::Korean] {
            let ctx = egui::Context::default();
            let mut app = App::new(&ctx, true);
            app.settings.ui_language = language;
            app.tab = PackageTab::Translation;
            let positions = rendered_text_positions(&mut app, &ctx);
            let tab = positions
                .iter()
                .find(|(text, _)| text == &language.text("Localization"))
                .unwrap()
                .1;
            let label = positions
                .iter()
                .find(|(text, _)| text == &language.text("Language"))
                .unwrap()
                .1;
            let selector = positions
                .iter()
                .find(|(text, rect)| text == "KOR" && rect.top() > 100.)
                .unwrap()
                .1;
            assert!(
                (tab.center().y - label.center().y).abs() < 2.,
                "{tab:?} {label:?}"
            );
            assert!(
                (tab.center().y - selector.center().y).abs() < 2.,
                "{tab:?} {selector:?}"
            );
        }
    }

    #[test]
    fn saved_launch_platform_defaults_and_unknown_values_preserve_other_settings() {
        let settings = Settings {
            game_folder: "D:/Games/Paladins".into(),
            homedir: "existing-profile".into(),
            launch_language: "KOR".into(),
            performance_enabled: true,
            rollback_input: true,
            ..Settings::default()
        };
        let expected = serde_json::to_value(&settings).unwrap();
        for value in [
            None,
            Some(serde_json::json!("future-platform")),
            Some(serde_json::json!(17)),
        ] {
            let mut legacy = expected.clone();
            if let Some(value) = value {
                legacy["launch_platform"] = value;
            } else {
                legacy.as_object_mut().unwrap().remove("launch_platform");
            }
            let restored: Settings = serde_json::from_value(legacy).unwrap();
            assert_eq!(serde_json::to_value(restored).unwrap(), expected);
        }
    }

    #[test]
    fn launch_platform_menu_is_beside_play_and_changes_only_the_passed_platform() {
        for language in [UiLanguage::English, UiLanguage::Korean] {
            let ctx = egui::Context::default();
            ctx.global_style_mut(|style| style.animation_time = 0.);
            let mut app = App::new(&ctx, true);
            app.settings.ui_language = language;
            app.settings.game_folder = "D:/Games/Paladins".into();
            app.settings.homedir = "existing-profile".into();
            let mut expected = serde_json::to_value(&app.settings).unwrap();
            let positions = rendered_text_positions(&mut app, &ctx);
            let selector = positions
                .iter()
                .find(|(text, _)| text == "PC (Hi-Rez)")
                .unwrap()
                .1;
            let play = positions
                .iter()
                .find(|(text, _)| text == &language.text("Play"))
                .unwrap()
                .1;
            assert!(
                (selector.center().y - play.center().y).abs() < 2.,
                "{selector:?} {play:?}"
            );
            assert!(selector.right() < play.left());
            for (platform, serialized) in [
                (launch::LaunchPlatform::Steam, "steam"),
                (launch::LaunchPlatform::Epic, "epic"),
                (launch::LaunchPlatform::Pc, "pc"),
            ] {
                let (_, trigger) = rendered_text_positions(&mut app, &ctx)
                    .into_iter()
                    .find(|(text, _)| text == app.settings.launch_platform.caption())
                    .unwrap();
                click_at_point(&mut app, &ctx, trigger.center());
                let (_, choice) = rendered_text_positions(&mut app, &ctx)
                    .into_iter()
                    .find(|(text, _)| text == platform.caption())
                    .unwrap();
                click_at_point(&mut app, &ctx, choice.center());
                assert_eq!(app.launch_options().platform, platform);
                expected["launch_platform"] = serde_json::json!(serialized);
                assert_eq!(serde_json::to_value(&app.settings).unwrap(), expected);
                let restored: Settings =
                    serde_json::from_slice(&serde_json::to_vec(&app.settings).unwrap()).unwrap();
                assert_eq!(restored.launch_platform, platform);
            }
        }
    }

    #[test]
    fn game_language_dropdown_changes_only_the_game_locale_and_preserves_saved_codes() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        app.settings.homedir = "existing-profile".into();
        let mut expected = serde_json::to_value(&app.settings).unwrap();
        let (_, choice) = rendered_text_positions(&mut app, &ctx)
            .into_iter()
            .find(|(text, _)| text == "English (INT)")
            .unwrap();
        click_at_point(&mut app, &ctx, choice.center());
        let (_, korean) = rendered_text_positions(&mut app, &ctx)
            .into_iter()
            .find(|(text, _)| text == "Korean (KOR)")
            .unwrap();
        click_at_point(&mut app, &ctx, korean.center());
        assert_eq!(app.launch_options().language, "KOR");
        assert_eq!(app.settings.ui_language, UiLanguage::English);
        expected["launch_language"] = serde_json::json!("KOR");
        assert_eq!(serde_json::to_value(&app.settings).unwrap(), expected);
        app.settings.launch_language = "FRA".into();
        assert!(app.game_language_choices().contains(&"FRA".to_owned()));
        assert!(rendered_text(&mut app, &ctx).contains("FRA"));
        assert_eq!(app.launch_options().language, "FRA");
    }

    #[test]
    fn header_language_menu_switches_ui_and_preserves_game_configuration() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        app.settings.homedir = "existing-profile".into();
        app.settings.launch_language = "INT".into();
        app.settings.prepared_translation = "existing/bundle.json".into();
        app.settings.prepared_ime = "existing/ime-package.json".into();
        let mut expected = serde_json::to_value(&app.settings).unwrap();
        rendered_text(&mut app, &ctx);
        let trigger = ctx.read_response(language_menu::button_id()).unwrap().rect;
        assert!(
            trigger.right() > 800. && trigger.bottom() < 80.,
            "Header control: {trigger:?}"
        );
        click_language_widget(&mut app, &ctx, language_menu::button_id());
        assert!(egui::Popup::is_id_open(&ctx, language_menu::popup_id()));
        assert!(
            ctx.read_response(language_menu::option_id(UiLanguage::English))
                .is_some()
        );
        assert!(
            ctx.read_response(language_menu::option_id(UiLanguage::Korean))
                .is_some()
        );
        click_language_widget(&mut app, &ctx, language_menu::option_id(UiLanguage::Korean));
        assert_eq!(app.settings.ui_language, UiLanguage::Korean);
        assert!(!egui::Popup::is_id_open(&ctx, language_menu::popup_id()));
        let text = rendered_text(&mut app, &ctx);
        assert!(
            text.contains("게임 설치 경로")
                && text.contains("성능 모드")
                && text.contains("준비됨"),
            "{text}"
        );
        expected["ui_language"] = serde_json::json!("KOR");
        assert_eq!(serde_json::to_value(&app.settings).unwrap(), expected);
        let reloaded: Settings = serde_json::from_value(expected).unwrap();
        assert_eq!(reloaded.ui_language, UiLanguage::Korean);
        assert_eq!(app.launch_options().language, "INT");
        assert_eq!(app.launch_options().homedir, "existing-profile");
        click_language_widget(&mut app, &ctx, language_menu::button_id());
        click_language_widget(
            &mut app,
            &ctx,
            language_menu::option_id(UiLanguage::English),
        );
        assert_eq!(app.settings.ui_language, UiLanguage::English);
        assert!(rendered_text(&mut app, &ctx).contains("Game installation"));
    }

    #[test]
    fn language_menu_dismisses_on_escape_and_outside_click() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        rendered_text(&mut app, &ctx);
        click_language_widget(&mut app, &ctx, language_menu::button_id());
        language_menu_frame(
            &mut app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(!egui::Popup::is_id_open(&ctx, language_menu::popup_id()));
        click_language_widget(&mut app, &ctx, language_menu::button_id());
        let point = egui::pos2(450., 28.);
        language_menu_frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        language_menu_frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(!egui::Popup::is_id_open(&ctx, language_menu::popup_id()));
        assert_eq!(app.settings.ui_language, UiLanguage::English);
    }

    #[test]
    fn combined_startup_errors_switch_each_caption_without_translating_diagnostics() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Event::Done(Ok(Work::Startup(Startup {
                prepared: PreparedPaths::default(),
                catalog: Err("catalog offline".into()),
                translation: Some(Err("C:/Games/Ready/bundle.json".into())),
                ime: Some(Err("native ABI mismatch".into())),
                performance: None,
                performance_release: None,
            }))))
            .unwrap();
        app.receiver = Some(receiver);
        app.receive();
        let korean = app.presented_status(UiLanguage::Korean);
        assert!(
            korean.contains("GitHub 카탈로그:")
                && korean.contains("저장된 번역 패키지:")
                && korean.contains("저장된 IME 패키지:"),
            "{korean}"
        );
        assert!(
            korean.contains("C:/Games/Ready/bundle.json") && korean.contains("native ABI mismatch")
        );
        assert_eq!(app.presented_status(UiLanguage::English), app.status);
        let (sender, receiver) = mpsc::channel();
        sender.send(Event::Progress("Preparing…".into())).unwrap();
        app.receiver = Some(receiver);
        app.receive();
        assert_eq!(app.presented_status(UiLanguage::Korean), "준비 중…");
    }

    #[test]
    fn launch_summary_stays_short_when_repository_settings_are_expanded() {
        for language in [UiLanguage::English, UiLanguage::Korean] {
            for observed in [true, false] {
                let ctx = egui::Context::default();
                let mut app = App::new(&ctx, true);
                ctx.global_style_mut(|style| style.animation_time = 0.);
                app.settings.ui_language = language;
                let diagnostic = "Game dispatched (PID 63628); fresh selected game observed (PID 63628). Mod initialization is not yet verified.";
                let (sender, receiver) = mpsc::channel();
                sender
                    .send(Event::Done(Ok(Work::Launch(launch::LaunchResult {
                        launcher_pid: 63628,
                        game_pid: observed.then_some(63628),
                        log: PathBuf::from("C:/logs/launch.log"),
                        status: diagnostic.into(),
                    }))))
                    .unwrap();
                app.receiver = Some(receiver);
                let expected = if observed {
                    "Game started"
                } else {
                    "Launch requested"
                };
                let text = rendered_text(&mut app, &ctx);
                assert!(text.contains(&language.text(expected)), "{text}");
                assert!(
                    !text.contains("63628") && !text.contains("initialization"),
                    "{text}"
                );
                assert_eq!(app.status_parts[0], diagnostic);
                let (_, disclosure) = rendered_text_positions(&mut app, &ctx)
                    .into_iter()
                    .find(|(text, _)| text == &language.text("Repository settings"))
                    .unwrap();
                click_at_point(&mut app, &ctx, disclosure.center());
                let text = rendered_text(&mut app, &ctx);
                assert!(
                    text.contains(&engine::data_dir().display().to_string()),
                    "{text}"
                );
                assert!(text.contains(&language.text("Copy log path")), "{text}");
                assert_eq!(text.matches(&language.text(expected)).count(), 1, "{text}");
                assert!(
                    !text.contains("63628")
                        && !text.contains("initialization")
                        && !text.contains("C:/logs/launch.log")
                        && !text.contains(&language.text("Details")),
                    "{text}"
                );
                let (_, diagnostics) = rendered_text_positions(&mut app, &ctx)
                    .into_iter()
                    .find(|(text, _)| text == &language.text("Diagnostics"))
                    .unwrap();
                click_at_point(&mut app, &ctx, diagnostics.center());
                let text = rendered_text(&mut app, &ctx);
                assert!(
                    text.contains(&language.text(diagnostic))
                        && text.contains("C:/logs/launch.log"),
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn installed_language_mismatch_has_one_localized_action_in_the_footer() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        app.status = launch::LOCALIZATION_LANGUAGE_MISMATCH.into();
        app.error = true;
        assert_eq!(app.concise_status(), launch::LOCALIZATION_LANGUAGE_MISMATCH);
        assert!(rendered_text(&mut app, &ctx).contains(launch::LOCALIZATION_LANGUAGE_MISMATCH));
        app.settings.ui_language = i18n::UiLanguage::Korean;
        assert!(rendered_text(&mut app, &ctx).contains("게임 언어를 설치된 패키지에 맞추세요"));
        app.status.push_str("; native details");
        assert_eq!(app.concise_status(), "Operation failed");
    }

    #[test]
    fn arbitrary_backend_errors_and_progress_never_reach_the_default_footer() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        let payload = "native PID 98765; C:/private/path; validation and provenance details";
        app.status = payload.into();
        assert_eq!(app.concise_status(), "Done");
        assert!(!rendered_text(&mut app, &ctx).contains(payload));
        app.error = true;
        let text = rendered_text(&mut app, &ctx);
        assert!(text.contains("Operation failed") && !text.contains(payload));
        app.error = false;
        let (sender, receiver) = mpsc::channel();
        sender.send(Event::Progress(payload.into())).unwrap();
        app.receiver = Some(receiver);
        let text = rendered_text(&mut app, &ctx);
        assert!(text.contains("Working…") && !text.contains(payload));
        assert_eq!(app.status, payload);
    }

    #[test]
    fn clearing_downloads_invalidates_prepared_packages_even_after_a_partial_error() {
        for language in [UiLanguage::English, UiLanguage::Korean] {
            for failed in [false, true] {
                let ctx = egui::Context::default();
                ctx.global_style_mut(|style| style.animation_time = 0.);
                let mut app = App::new(&ctx, true);
                app.settings.ui_language = language;
                app.settings.game_folder = "C:/Games/preserved-installation".into();
                app.settings.launch_language = "FRA".into();
                app.settings.homedir = "preserved-profile".into();
                app.settings.performance_enabled = true;
                app.settings.rollback_input = true;
                app.settings.prepared_translation = "cache/bundles/bundle.json".into();
                app.settings.prepared_ime = "cache/ime/ime-package.json".into();
                app.settings.prepared_performance = "cache/performance/manifest.json".into();
                app.installed = "KOR · installed-package".into();
                app.ime_installed =
                    ImeInstallState::Enabled(Box::new(ime_package("0.1.0").package));
                app.selected = Some(Cached {
                    package: serde_json::from_value(serde_json::json!({
                        "version": 1,
                        "bundle_id": "prepared-translation",
                        "locale": "KOR",
                        "files": []
                    }))
                    .unwrap(),
                    directory: PathBuf::from("cache/bundles"),
                });
                app.ime_selected = Some(ime_package("0.2.0"));
                app.performance_selected = Some(performance::Cached {
                    package: performance::Package {
                        schema: 1,
                        version: "0.1.0".into(),
                        mod_id: "prepared-performance".into(),
                        platform: "Win64".into(),
                        source_revision: "c".repeat(40),
                        game_sha256: "a".repeat(64),
                        dinput8_sha256: "b".repeat(64),
                        user32_sha256: "d".repeat(64),
                        performance_verified: false,
                        compatibility_verified: false,
                        rendering: "fixture".into(),
                        worker_policy: None,
                        assets: Vec::new(),
                    },
                    directory: PathBuf::from("cache/performance"),
                });
                app.accept_catalog(catalog_fixture(true, true));
                let installed = app.installed.clone();
                let installed_ime = app.ime_installed.label();
                let mut expected = serde_json::to_value(&app.settings).unwrap();
                for key in [
                    "prepared_translation",
                    "prepared_ime",
                    "prepared_performance",
                ] {
                    expected[key] = serde_json::json!("");
                }
                let diagnostic = "Recycle Bin refused C:/cache/bundles (os error 5)";
                let result = if failed {
                    Err(diagnostic.into())
                } else {
                    Ok(())
                };
                let (sender, receiver) = mpsc::channel();
                sender
                    .send(Event::Done(Ok(Work::DownloadsCleared(result))))
                    .unwrap();
                app.receiver = Some(receiver);
                app.receive();

                assert!(app.selected.is_none());
                assert!(app.ime_selected.is_none());
                assert!(app.performance_selected.is_none());
                assert_eq!(serde_json::to_value(&app.settings).unwrap(), expected);
                assert_eq!(app.installed, installed);
                assert_eq!(app.ime_installed.label(), installed_ime);
                assert!(app.ime_installed.enabled());
                assert_eq!(app.releases.len(), 1);
                assert_eq!(app.ime_releases.len(), 1);
                assert_eq!(app.error, failed);
                assert!(app.receiver.is_none());
                let concise = if failed {
                    "Operation failed"
                } else {
                    "Downloads cleared"
                };
                assert_eq!(app.concise_status(), concise);
                let text = rendered_text(&mut app, &ctx);
                assert!(text.contains(&language.text(concise)), "{text}");
                assert!(!text.contains(diagnostic), "{text}");
                if failed {
                    let (_, disclosure) = rendered_text_positions(&mut app, &ctx)
                        .into_iter()
                        .find(|(text, _)| text == &language.text("Repository settings"))
                        .unwrap();
                    click_at_point(&mut app, &ctx, disclosure.center());
                    assert!(!rendered_text(&mut app, &ctx).contains(diagnostic));
                    let (_, diagnostics) = rendered_text_positions(&mut app, &ctx)
                        .into_iter()
                        .find(|(text, _)| text == &language.text("Diagnostics"))
                        .unwrap();
                    click_at_point(&mut app, &ctx, diagnostics.center());
                    let text = rendered_text(&mut app, &ctx);
                    assert!(text.contains(diagnostic), "{text}");
                    assert!(
                        text.contains(&language.text("Could not clear downloads: ")),
                        "{text}"
                    );
                }
            }
        }
    }

    #[test]
    fn local_data_path_and_approved_repo_wording_show_on_every_expanded_tab() {
        for language in [UiLanguage::English, UiLanguage::Korean] {
            for tab in [
                PackageTab::Performance,
                PackageTab::Translation,
                PackageTab::Ime,
            ] {
                let ctx = egui::Context::default();
                ctx.global_style_mut(|style| style.animation_time = 0.);
                let mut app = App::new(&ctx, true);
                app.settings.ui_language = language;
                app.tab = tab;
                let path = engine::data_dir().display().to_string();
                assert!(!rendered_text(&mut app, &ctx).contains(&path));
                let caption = match language {
                    UiLanguage::English => "Repository settings",
                    UiLanguage::Korean => "레포 설정",
                };
                let (_, disclosure) = rendered_text_positions(&mut app, &ctx)
                    .into_iter()
                    .find(|(text, _)| text == caption)
                    .unwrap();
                click_at_point(&mut app, &ctx, disclosure.center());
                let text = rendered_text(&mut app, &ctx);
                assert!(text.contains(&path), "{text}");
                assert!(text.contains(&language.text("Local path")), "{text}");
                assert!(
                    !text.contains("저장소") && !text.contains(&language.text("Details")),
                    "{text}"
                );
                if language == UiLanguage::Korean && tab != PackageTab::Performance {
                    assert!(text.contains("GitHub 레포"), "{text}");
                }
            }
        }
    }

    #[test]
    fn copy_log_path_emits_the_exact_unicode_path_without_opening_a_file() {
        let ctx = egui::Context::default();
        ctx.global_style_mut(|style| style.animation_time = 0.);
        let mut app = App::new(&ctx, true);
        app.settings.ui_language = UiLanguage::Korean;
        app.preview_repository_open = true;
        let log = r"C:\Users\tester\로그 자료\launch,123.log";
        app.launch_log = Some(PathBuf::from(log));
        let (_, button) = rendered_text_positions(&mut app, &ctx)
            .into_iter()
            .find(|(text, _)| text == "로그 경로 복사")
            .unwrap();
        let point = button.center();
        language_menu_frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(880., 960.),
                )),
                events: vec![egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| app.render(ui),
        );
        output.textures_delta.clear();
        assert!(output.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(value) if value == log)
        }));
        assert!(
            !output
                .platform_output
                .commands
                .iter()
                .any(|command| { matches!(command, egui::OutputCommand::OpenUrl(_)) })
        );
    }

    #[test]
    fn clear_downloads_is_visible_at_bottom_with_repository_details_closed() {
        for language in [UiLanguage::English, UiLanguage::Korean] {
            for tab in [
                PackageTab::Translation,
                PackageTab::Ime,
                PackageTab::Performance,
            ] {
                let ctx = egui::Context::default();
                let mut app = App::new(&ctx, true);
                app.settings.ui_language = language;
                app.tab = tab;
                let positions = rendered_text_positions(&mut app, &ctx);
                let (_, button) = positions
                    .iter()
                    .find(|(text, _)| text == &language.text("Clear downloads"))
                    .unwrap();
                let (_, disclosure) = positions
                    .iter()
                    .find(|(text, _)| text == &language.text("Repository settings"))
                    .unwrap();
                assert!(
                    (button.center().y - disclosure.center().y).abs() < 3.,
                    "Button: {button:?}, disclosure: {disclosure:?}"
                );
                assert!(
                    button.left() > disclosure.right(),
                    "Button: {button:?}, disclosure: {disclosure:?}"
                );
                assert!(
                    button.top() > 850. && button.right() > 740.,
                    "Button: {button:?}"
                );
                assert!(
                    !positions
                        .iter()
                        .any(|(text, _)| text == REPOSITORY || text == performance::REPOSITORY)
                );
            }
        }
    }

    #[test]
    fn clear_downloads_control_is_disabled_during_other_work() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        for busy in [false, true] {
            let mut enabled = None;
            let mut frame = ctx.run_ui(egui::RawInput::default(), |ui| {
                enabled = Some(app.clear_downloads_button(ui, &ctx, busy).enabled());
            });
            frame.textures_delta.clear();
            assert_eq!(enabled, Some(!busy));
        }
        let (_sender, receiver) = mpsc::channel();
        app.receiver = Some(receiver);
        let mut enabled = None;
        let mut frame = ctx.run_ui(egui::RawInput::default(), |ui| {
            enabled = Some(app.clear_downloads_button(ui, &ctx, false).enabled());
        });
        frame.textures_delta.clear();
        assert_eq!(enabled, Some(false));
    }

    #[test]
    fn failed_saved_package_imports_forget_only_the_matching_prepared_family() {
        for failed_tab in [
            PackageTab::Translation,
            PackageTab::Ime,
            PackageTab::Performance,
        ] {
            let ctx = egui::Context::default();
            let mut app = App::new(&ctx, true);
            app.settings.game_folder = "C:/Games/preserved-installation".into();
            app.settings.launch_language = "FRA".into();
            app.settings.homedir = "preserved-profile".into();
            app.settings.performance_enabled = true;
            app.settings.rollback_input = true;
            app.settings.prepared_translation = "cache/bundles/bundle.json".into();
            let mut stale_ime = ime_package("0.1.0");
            stale_ime.directory = PathBuf::from("cache/ime");
            app.accept_ime(stale_ime);
            app.settings.prepared_performance = "cache/performance/manifest.json".into();
            let prepared = PreparedPaths {
                translation: app.settings.prepared_translation.clone(),
                ime: app.settings.prepared_ime.clone(),
                performance: app.settings.prepared_performance.clone(),
            };
            let mut expected = serde_json::to_value(&app.settings).unwrap();
            expected[match failed_tab {
                PackageTab::Translation => "prepared_translation",
                PackageTab::Ime => "prepared_ime",
                PackageTab::Performance => "prepared_performance",
                PackageTab::Mods => unreachable!(),
            }] = serde_json::json!("");
            let (sender, receiver) = mpsc::channel();
            sender
                .send(Event::Done(Ok(Work::Startup(Startup {
                    prepared,
                    catalog: Ok(catalog_fixture(true, true)),
                    translation: (failed_tab == PackageTab::Translation)
                        .then(|| Err("Retired translation cache".into())),
                    ime: (failed_tab == PackageTab::Ime).then(|| Err("Retired IME cache".into())),
                    performance: (failed_tab == PackageTab::Performance)
                        .then(|| Err("Retired performance cache".into())),
                    performance_release: None,
                }))))
                .unwrap();
            app.receiver = Some(receiver);
            app.receive();
            assert_eq!(serde_json::to_value(&app.settings).unwrap(), expected);
            assert_eq!(app.ime_selected.is_none(), failed_tab == PackageTab::Ime);
            assert!(app.error);
        }
    }

    #[test]
    fn old_startup_failure_cannot_forget_a_newer_prepared_selection() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, true);
        let mut current = ime_package("0.2.0");
        current.directory = PathBuf::from("cache/current-ime");
        app.accept_ime(current);
        let expected = serde_json::to_value(&app.settings).unwrap();
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Event::Done(Ok(Work::Startup(Startup {
                prepared: PreparedPaths {
                    ime: "cache/retired-ime/ime-package.json".into(),
                    ..Default::default()
                },
                catalog: Ok(catalog_fixture(true, true)),
                translation: None,
                ime: Some(Err("Retired IME cache".into())),
                performance: None,
                performance_release: None,
            }))))
            .unwrap();
        app.receiver = Some(receiver);
        app.receive();
        assert_eq!(serde_json::to_value(&app.settings).unwrap(), expected);
        assert_eq!(app.ime_selected.as_ref().unwrap().package.version, "0.2.0");
    }

    #[test]
    fn package_success_keeps_new_selection_when_old_cache_cleanup_fails() {
        for language in [UiLanguage::English, UiLanguage::Korean] {
            let ctx = egui::Context::default();
            ctx.global_style_mut(|style| style.animation_time = 0.);
            let mut app = App::new(&ctx, true);
            app.settings.ui_language = language;
            app.settings.launch_language = "INT".into();
            app.settings.homedir = "preserved-profile".into();
            let package = |id: &str, directory: &str| Cached {
                package: serde_json::from_value(serde_json::json!({
                    "version": 1,
                    "bundle_id": id,
                    "locale": "KOR",
                    "files": []
                }))
                .unwrap(),
                directory: PathBuf::from(directory),
            };
            app.accept_translation(package("old-package", "cache/old"));
            let newer = package("verified-new-package", "cache/new");
            let mut expected = serde_json::to_value(&app.settings).unwrap();
            expected["prepared_translation"] =
                serde_json::json!(newer.directory.join("bundle.json").display().to_string());
            let warning = "Recycle Bin refused old C:/cache/bundles (os error 5)";
            let (sender, receiver) = mpsc::channel();
            sender
                .send(Event::Completion(
                    Ok(Work::Package(Box::new(newer))),
                    Some(warning.into()),
                ))
                .unwrap();
            app.receiver = Some(receiver);
            app.receive();
            assert_eq!(
                app.selected.as_ref().unwrap().package.bundle_id,
                "verified-new-package"
            );
            assert_eq!(serde_json::to_value(&app.settings).unwrap(), expected);
            assert_eq!(app.concise_status(), "Package ready");
            assert!(!app.error);
            assert!(app.status_parts[0].contains(warning));
            let text = rendered_text(&mut app, &ctx);
            assert!(text.contains(&language.text("Package ready")), "{text}");
            assert!(!text.contains(warning), "{text}");
            let (_, disclosure) = rendered_text_positions(&mut app, &ctx)
                .into_iter()
                .find(|(text, _)| text == &language.text("Repository settings"))
                .unwrap();
            click_at_point(&mut app, &ctx, disclosure.center());
            assert!(!rendered_text(&mut app, &ctx).contains(warning));
            let (_, diagnostics) = rendered_text_positions(&mut app, &ctx)
                .into_iter()
                .find(|(text, _)| text == &language.text("Diagnostics"))
                .unwrap();
            click_at_point(&mut app, &ctx, diagnostics.center());
            let text = rendered_text(&mut app, &ctx);
            assert!(text.contains(warning), "{text}");
            assert!(
                text.contains(&language.text("Download cache cleanup: ")),
                "{text}"
            );
        }
    }
}
