//! Explicit, finite standalone game dispatch with compatible Tempest mod metadata.
//!
//! Launch/profile and enabled V2-mod discovery are adapted from LowRezStudio/Tempest
//! at efe828dd28a48c4e1861ebf25a99cc16bd4b2425:
//! Tempest.CLI/Launcher/{LauncherCommands,LauncherUtility}.cs,
//! Tempest.CLI/Mods/ModCommands.cs and Tempest.CLI/TempestPathUtility.cs.
//! Upstream source: https://github.com/LowRezStudio/Tempest (GPL-3.0-or-later).
//! This backend never changes existing mod metadata, kills an existing client,
//! or treats dispatch as proof that hooks are active.

use crate::{engine, ime, performance, upk_patcher};
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
#[path = "native_launch.rs"]
mod native;

const GAME: &str = "Binaries/Win64/Paladins.exe";
const ENGINE_CONFIGS: [&str; 2] = [
    "Engine/Config/BaseEngine.ini",
    "ChaosGame/Config/DefaultEngine.ini",
];
const REGISTRY: &str = ".tempest/mods/mods.json";
const MAX_METADATA: u64 = 4 * 1024 * 1024;
const MAX_MODS: usize = 4096;
const MAX_DISCOVERED_DLLS: usize = 512;
const OBSERVATION: Duration = Duration::from_secs(8);

pub const DEFAULT_LANGUAGE: &str = "INT";
pub const DEFAULT_PROFILE: &str = "PaladinsCat";
pub const LOCALIZATION_LANGUAGE_MISMATCH: &str = "Match Game language to the installed package";

/// Login flags match Tempest's instance wizard/onboarding pass-through contract.
/// This is independent of the game's installation store and Win64 architecture.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LaunchPlatform {
    #[default]
    Pc,
    Steam,
    Epic,
}
impl LaunchPlatform {
    pub const ALL: [Self; 3] = [Self::Pc, Self::Steam, Self::Epic];
    pub const fn caption(self) -> &'static str {
        match self {
            Self::Pc => "PC (Hi-Rez)",
            Self::Steam => "Steam",
            Self::Epic => "Epic",
        }
    }
    pub fn parse(value: &str) -> engine::Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "pc" | "hirez" => Ok(Self::Pc),
            "steam" => Ok(Self::Steam),
            "epic" => Ok(Self::Epic),
            _ => Err("Launch platform must be pc, steam or epic".into()),
        }
    }
    fn flag(self) -> Option<&'static str> {
        match self {
            Self::Pc => None,
            Self::Steam => Some("-steam"),
            Self::Epic => Some("-epic"),
        }
    }
}
impl<'de> Deserialize<'de> for LaunchPlatform {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        // An unsupported saved choice must not discard the rest of Settings.
        Ok(value
            .as_str()
            .and_then(|s| Self::parse(s).ok())
            .unwrap_or_default())
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub game_root: PathBuf,
    pub language: String,
    pub homedir: String,
    pub platform: LaunchPlatform,
    pub performance: Option<performance::Cached>,
    pub rollback_input: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub args: Vec<String>,
    pub game_root: PathBuf,
    pub game_executable: PathBuf,
    pub working_directory: PathBuf,
    /// Verified managed payloads and enabled compatible registry DLLs, loaded once.
    pub dlls: Vec<PathBuf>,
    dll_hashes: Vec<String>,
    registry: Option<(PathBuf, String)>,
    engine_configs: Vec<(PathBuf, String)>,
}

#[derive(Clone, Debug)]
pub struct LaunchResult {
    pub launcher_pid: u32,
    pub game_pid: Option<u32>,
    pub log: PathBuf,
    pub status: String,
}

fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn argument_path(path: &Path) -> engine::Result<String> {
    let value = path.to_str().ok_or("Launch paths must be Unicode")?;
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err("Launch paths cannot contain control characters".into());
    }
    // Canonical paths are retained for identity, containment and file pins. UE3
    // and the external loader receive ordinary Win32 paths, not verbatim syntax.
    if let Some(tail) = value.strip_prefix(r"\\?\UNC\") {
        let mut parts = tail.split('\\');
        if parts.next().is_none_or(str::is_empty) || parts.next().is_none_or(str::is_empty) {
            return Err("Launch UNC paths require a server and share".into());
        }
        return Ok(format!(r"\\{tail}"));
    }
    if let Some(tail) = value.strip_prefix(r"\\?\") {
        let bytes = tail.as_bytes();
        if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1..3] == *b":\\" {
            return Ok(tail.to_owned());
        }
        return Err("Launch paths must use a drive or UNC share, not a device namespace".into());
    }
    if value.starts_with(r"\\.\") {
        return Err("Launch paths cannot use a device namespace".into());
    }
    Ok(value.to_owned())
}

fn path_key(path: &Path) -> engine::Result<String> {
    // Process discovery and TEMP can use 8.3 aliases; plans use final paths.
    // Resolve both before comparing, retaining failure for inaccessible paths.
    let value = argument_path(&path.canonicalize().map_err(error)?)?;
    Ok(value.replace('/', "\\").to_lowercase())
}

fn external_path(path: &Path) -> engine::Result<String> {
    let value = argument_path(path)?;
    // Removing verbatim syntax must not redirect a trailing-dot/space component
    // to a different ordinary Win32 file. Resolve both before crossing the boundary.
    if path.canonicalize().map_err(error)? != Path::new(&value).canonicalize().map_err(error)? {
        return Err("Launch path changes filesystem identity in ordinary path syntax".into());
    }
    Ok(value)
}

fn inside(root: &Path, path: &Path) -> engine::Result<PathBuf> {
    let path = path.canonicalize().map_err(error)?;
    let root_key = path_key(root)?;
    let path_key = path_key(&path)?;
    if path_key != root_key && !path_key.starts_with(&format!("{root_key}\\")) {
        return Err(format!(
            "Launch path escapes selected game: {}",
            path.display()
        ));
    }
    Ok(path)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> engine::Result<T> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(error)?
        .take(MAX_METADATA + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 > MAX_METADATA {
        return Err("Launch metadata exceeds the size limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("Invalid {}: {e}", path.display()))
}

#[derive(Deserialize)]
struct LocaleReceipt {
    version: u32,
    locale: String,
    game_root: PathBuf,
    applied: bool,
    files: Vec<LocaleFile>,
}
#[derive(Deserialize)]
struct LocaleFile {
    target: PathBuf,
    backup: PathBuf,
    stock_sha256: String,
    artifact_sha256: String,
}

fn verify_state(root: &Path, locale: &str) -> engine::Result<()> {
    let state = root.join(".watchcat-localization");
    if state.exists() {
        inside(root, &state)?;
    }
    if engine::recovery_pending(root) || ime::recovery_pending(root) {
        return Err("Recover the interrupted localization/IME operation before launching".into());
    }
    let receipt = state.join("active.json");
    if !receipt.exists() {
        return Ok(());
    }
    let receipt: LocaleReceipt = read_json(&inside(root, &receipt)?)?;
    if receipt.version != 1
        || !receipt.applied
        || path_key(&receipt.game_root.canonicalize().map_err(error)?)? != path_key(root)?
        || receipt.files.is_empty()
        || receipt.files.len() > MAX_MODS
    {
        return Err("Installed localization receipt requires recovery".into());
    }
    let state = state.canonicalize().map_err(error)?;
    let mut targets = HashSet::new();
    for file in receipt.files {
        let target = inside(root, &file.target)?;
        if !targets.insert(path_key(&target)?) {
            return Err("Installed localization receipt repeats a target".into());
        }
        let backup = inside(&state, &file.backup)?;
        upk_patcher::require_hash(&target, &file.artifact_sha256, "installed localization")?;
        upk_patcher::require_hash(&backup, &file.stock_sha256, "localization rollback backup")?;
    }
    if receipt.locale != locale {
        return Err(LOCALIZATION_LANGUAGE_MISMATCH.into());
    }
    Ok(())
}

fn validate_profile(options: &Options) -> engine::Result<()> {
    if options.language.len() != 3 || !options.language.bytes().all(|c| c.is_ascii_uppercase()) {
        return Err(
            "Language must be a three-letter uppercase game locale, e.g. INT or KOR".into(),
        );
    }
    if options.homedir.is_empty()
        || options.homedir.len() > 64
        || !options
            .homedir
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        return Err(
            "Home profile must contain 1–64 letters, digits, hyphens or underscores".into(),
        );
    }
    if options.rollback_input && options.performance.is_none() {
        return Err("Input rollback requires the selected performance package".into());
    }
    if !options.game_root.is_absolute() {
        return Err("Game path must be absolute".into());
    }
    argument_path(&options.game_root)?;
    Ok(())
}

/// Read-only plan: folders never enable a feature; IME requires its verified receipt,
/// and performance requires an explicitly selected, verified cached package.
pub fn plan(options: &Options) -> engine::Result<Plan> {
    plan_with(options, |options, root| {
        validate_game_image(&root.join(GAME))?;
        let mut selected = Vec::new();
        if let Some(package) = ime::installed_package(root)? {
            upk_patcher::require_hash(
                &root.join(GAME),
                &package.game_fingerprint.sha256,
                "IME game executable",
            )?;
            selected.push(inside(
                root,
                &root.join(ime::TARGET).join("dlls").join(ime::DLL),
            )?);
        }
        if let Some(package) = &options.performance {
            let system = std::env::var_os("SystemRoot")
                .map(PathBuf::from)
                .ok_or("Windows SystemRoot is unavailable")?
                .join("System32");
            performance::preflight(package, root, &system)?;
            for dll in performance::dlls(package, options.rollback_input)? {
                selected.push(dll.canonicalize().map_err(error)?);
            }
        }
        Ok(selected)
    })
}

fn validate_game_image(path: &Path) -> engine::Result<()> {
    use std::io::{Seek, SeekFrom};
    let mut file = File::open(path).map_err(error)?;
    let mut dos = [0u8; 64];
    let mut header = [0u8; 26];
    file.read_exact(&mut dos)
        .map_err(|_| "Supported game executable requires a DOS header")?;
    let offset = u32::from_le_bytes(dos[60..64].try_into().unwrap());
    if dos[..2] != *b"MZ" || !(64..=16 * 1024 * 1024).contains(&offset) {
        return Err("Supported game executable has an invalid DOS header".into());
    }
    file.seek(SeekFrom::Start(u64::from(offset)))
        .map_err(error)?;
    file.read_exact(&mut header)
        .map_err(|_| "Supported game executable has a truncated PE header")?;
    if header[..4] != *b"PE\0\0" || header[4..6] != [0x64, 0x86] || header[24..26] != [0x0b, 0x02] {
        return Err("Supported game executable must be Windows x64 PE32+".into());
    }
    Ok(())
}

fn plan_with(
    options: &Options,
    select: impl FnOnce(&Options, &Path) -> engine::Result<Vec<PathBuf>>,
) -> engine::Result<Plan> {
    validate_profile(options)?;
    let root = options.game_root.canonicalize().map_err(error)?;
    if crate::mods::recovery_pending(&root) {
        return Err("Recover the interrupted mod import before launching".into());
    }
    for folder in ["Binaries", "Engine", "ChaosGame"] {
        if !root.join(folder).is_dir() {
            return Err("Select the game root containing Binaries, Engine and ChaosGame".into());
        }
        inside(&root, &root.join(folder))?;
    }
    let executable = inside(&root, &root.join(GAME))?;
    if !executable.is_file() {
        return Err("The selected Win64 game executable is missing".into());
    }
    let working_directory = executable
        .parent()
        .ok_or("Game executable has no working directory")?
        .to_path_buf();
    let mut engine_configs = Vec::new();
    for relative in ENGINE_CONFIGS {
        let path = inside(&root, &root.join(relative))
            .map_err(|e| format!("Required startup configuration {relative}: {e}"))?;
        let metadata = fs::metadata(&path).map_err(error)?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(format!(
                "Required startup configuration {relative} is missing or empty"
            ));
        }
        let sha = upk_patcher::sha256_file(&path)?;
        engine_configs.push((path, sha));
    }
    verify_state(&root, &options.language)?;
    let selected = select(options, &root)?;
    let mut unique = HashSet::new();
    for dll in &selected {
        if !dll.is_absolute() || !dll.is_file() || !unique.insert(path_key(dll)?) {
            return Err("Selected DLL paths must be existing, absolute and unique".into());
        }
    }
    let automatic = audit_registry(&root, &selected)?;
    let registry = if root.join(REGISTRY).exists() {
        let path = inside(&root, &root.join(REGISTRY))?;
        let sha = upk_patcher::sha256_file(&path)?;
        Some((path, sha))
    } else {
        None
    };
    let automatic_keys = automatic
        .iter()
        .map(|dll| path_key(dll))
        .collect::<engine::Result<HashSet<_>>>()?;
    let mut dlls = selected
        .into_iter()
        .filter(|dll| !automatic_keys.contains(&path_key(dll).expect("selected paths validated")))
        .collect::<Vec<_>>();
    dlls.extend(automatic);
    // Paths are never marshalled through a comma-delimited external CLI.
    for dll in &dlls {
        external_path(dll)?;
    }
    // Compatible registry selection is user metadata, but final dispatch must
    // still bind every DLL to the bytes reviewed before read-only pins.
    let dll_hashes = dlls
        .iter()
        .map(|dll| upk_patcher::sha256_file(dll))
        .collect::<engine::Result<_>>()?;
    let mut args = vec![
        format!("-homedir={}", options.homedir),
        "-EACALT".into(),
        "-eac-nop-loaded".into(),
        "-COOKFORDEMO".into(),
        "-seekfreeloadingpcconsole".into(),
        "-pid=402".into(),
        "-replayfile=".into(),
        format!("-Language={}", options.language),
    ];
    if let Some(flag) = options.platform.flag() {
        args.insert(0, flag.into());
    }
    if let Some(package) = &options.performance {
        performance::apply_worker_policy(&package.package, &mut args)?;
    }
    Ok(Plan {
        args,
        game_root: root,
        game_executable: executable,
        working_directory,
        dlls,
        dll_hashes,
        registry,
        engine_configs,
    })
}

fn enabled_default() -> bool {
    true
}

/// PascalCase and Enabled=true reflect upstream System.Text.Json / ModRecord defaults.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TempestMod {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default = "enabled_default")]
    enabled: bool,
    #[serde(default)]
    installed_files: Vec<PathBuf>,
    #[serde(default)]
    owned_files: Vec<PathBuf>,
}

pub(crate) fn mod_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
}

pub(crate) fn family(value: &str) -> bool {
    let name: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    [
        "paladinscatime",
        "watchcatime",
        "imecompat",
        "paladinscatperformance",
        "paladinsperformance",
        "watchcatnativeinput",
        "watchcatworldscheduler",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

fn discover(root: &Path, dir: &Path, depth: usize, dlls: &mut Vec<PathBuf>) -> engine::Result<()> {
    if depth > 16 || dlls.len() > MAX_DISCOVERED_DLLS {
        return Err("Tempest mod DLL inventory exceeds the bounded depth/count".into());
    }
    let dir = inside(root, dir)?;
    for entry in fs::read_dir(dir).map_err(error)? {
        let path = entry.map_err(error)?.path();
        let metadata = fs::symlink_metadata(&path).map_err(error)?;
        // Junction/symlink traversal can create cycles or escape the selected installation.
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = metadata.file_type().is_symlink();
        if reparse {
            return Err("Tempest mod DLL inventory contains a junction or symbolic link".into());
        }
        if metadata.is_dir() {
            discover(root, &path, depth + 1, dlls)?;
        } else if path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("dll"))
        {
            dlls.push(inside(root, &path)?);
            if dlls.len() > MAX_DISCOVERED_DLLS {
                return Err("Tempest mod DLL inventory exceeds the bounded count".into());
            }
        }
    }
    Ok(())
}

/// Return enabled compatible registry DLLs in registry order, once per path.
fn audit_registry(root: &Path, selected: &[PathBuf]) -> engine::Result<Vec<PathBuf>> {
    let registry = root.join(REGISTRY);
    if !registry.exists() {
        return Ok(Vec::new());
    }
    let mods: Vec<TempestMod> = read_json(&inside(root, &registry)?)?;
    if mods.len() > MAX_MODS {
        return Err("Tempest mod registry exceeds the count limit".into());
    }
    let selected: HashSet<_> = selected
        .iter()
        .map(|p| path_key(p))
        .collect::<engine::Result<_>>()?;
    let mut ids = HashSet::new();
    let mut automatic = HashSet::new();
    let mut paths = Vec::new();
    let mut discovered_count = 0;
    for record in mods {
        if !record.id.is_empty() && !ids.insert(record.id.to_lowercase()) {
            return Err("Tempest mod registry repeats an ID; resolve it before launching".into());
        }
        if !record.enabled {
            continue;
        }
        let managed = family(&record.id) || family(&record.name);
        if managed
            && (!record.kind.eq_ignore_ascii_case("V2")
                || !record.id.eq_ignore_ascii_case(ime::MOD_ID))
        {
            return Err(format!(
                "Conflicting manually registered managed mod: {}",
                record.id
            ));
        }
        for owned in record
            .installed_files
            .iter()
            .chain(record.owned_files.iter())
        {
            if owned
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(family)
            {
                let owned = if owned.is_absolute() {
                    owned.clone()
                } else {
                    root.join(owned)
                };
                let owned = inside(root, &owned)?;
                if !record.id.eq_ignore_ascii_case(ime::MOD_ID)
                    || !selected.contains(&path_key(&owned)?)
                {
                    return Err(format!(
                        "Conflicting native DLL owner in Tempest registry: {}",
                        record.id
                    ));
                }
            }
        }
        if !record.kind.eq_ignore_ascii_case("V2") {
            continue;
        }
        if !mod_id(&record.id) {
            return Err("Enabled Tempest V2 mod has an unsafe/missing ID".into());
        }
        let dir = root.join(".tempest/v2/mods").join(&record.id).join("dlls");
        if !dir.exists() {
            if managed {
                return Err("Registered IME mod has no verified selected DLL".into());
            }
            continue;
        }
        let mut dlls = Vec::new();
        discover(root, &dir, 0, &mut dlls)?;
        dlls.sort_by_cached_key(|path| path.to_string_lossy().to_lowercase());
        discovered_count += dlls.len();
        if discovered_count > MAX_DISCOVERED_DLLS {
            return Err("Tempest enabled DLL inventory exceeds count limit".into());
        }
        if managed && dlls.is_empty() {
            return Err("Registered IME mod has no verified selected DLL".into());
        }
        for dll in dlls {
            let key = path_key(&dll)?;
            let managed_dll = dll.file_name().and_then(|s| s.to_str()).is_some_and(family);
            if (managed || managed_dll)
                && (!record.id.eq_ignore_ascii_case(ime::MOD_ID) || !selected.contains(&key))
            {
                return Err(format!(
                    "Conflicting enabled native mod DLL: {}",
                    dll.display()
                ));
            }
            if !automatic.insert(key) {
                return Err("Enabled registry would load the same DLL more than once".into());
            }
            paths.push(dll);
        }
    }
    Ok(paths)
}

#[derive(Clone, Debug)]
struct GameProcess {
    pid: u32,
    executable: Option<PathBuf>,
}

trait Dispatch {
    fn id(&self) -> u32;
    fn poll(&mut self) -> engine::Result<Option<i32>>;
}
impl Dispatch for Child {
    fn id(&self) -> u32 {
        Child::id(self)
    }
    fn poll(&mut self) -> engine::Result<Option<i32>> {
        Ok(self
            .try_wait()
            .map_err(error)?
            .map(|s| s.code().unwrap_or(-1)))
    }
}

trait Host {
    fn inventory(&self) -> engine::Result<Vec<GameProcess>>;
    fn dispatch(&self, plan: &Plan, log: &Path) -> engine::Result<Box<dyn Dispatch>>;
    fn pause(&self);
}
struct WindowsHost;
fn launch_command(plan: &Plan) -> engine::Result<Command> {
    let mut command = Command::new(external_path(&plan.game_executable)?);
    command
        .args(&plan.args)
        .current_dir(external_path(&plan.working_directory)?)
        .env("OPENSSL_ia32cap", "~0x20000000");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
    }
    Ok(command)
}
impl Host for WindowsHost {
    fn inventory(&self) -> engine::Result<Vec<GameProcess>> {
        game_processes()
    }
    fn dispatch(&self, plan: &Plan, log: &Path) -> engine::Result<Box<dyn Dispatch>> {
        let out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(log)
            .map_err(error)?;
        let err = out.try_clone().map_err(error)?;
        let mut command = launch_command(plan)?;
        command.stdin(Stdio::null()).stdout(out).stderr(err);
        #[cfg(windows)]
        {
            Ok(Box::new(native::dispatch(&mut command, &plan.dlls)?))
        }
        #[cfg(not(windows))]
        {
            Err("Game launch is supported on Windows only".into())
        }
    }
    fn pause(&self) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn ensure_idle(processes: &[GameProcess]) -> engine::Result<()> {
    if processes.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Paladins is already running (PIDs {}). This launcher leaves every existing client intact; close the intended client before Play.",
            processes
                .iter()
                .map(|p| p.pid.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

fn pinned_handles(plan: &Plan) -> engine::Result<Vec<File>> {
    std::iter::once(&plan.game_executable)
        .chain(plan.dlls.iter())
        .chain(plan.registry.iter().map(|(path, _)| path))
        .chain(plan.engine_configs.iter().map(|(path, _)| path))
        .map(|path| {
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(1); // Keep verified files read-only until startup observation.
            }
            options.open(path).map_err(error)
        })
        .collect()
}

/// Call only from an explicit Play action or an explicit CLI --launch request.
/// Observation is bounded; the caller receives pending status if startup takes longer.
pub fn start(options: &Options, log_dir: &Path) -> engine::Result<LaunchResult> {
    start_with(
        options,
        log_dir,
        &WindowsHost,
        || plan(options),
        OBSERVATION,
    )
}

fn start_with(
    options: &Options,
    log_dir: &Path,
    host: &dyn Host,
    planner: impl Fn() -> engine::Result<Plan>,
    observation: Duration,
) -> engine::Result<LaunchResult> {
    ensure_idle(&host.inventory()?)?;
    // Share the patch/install lock so another launcher or installer cannot race Play.
    let (_root, _lock) = engine::acquire(&options.game_root, &|| {
        host.inventory().map(|p| !p.is_empty()).unwrap_or(true)
    })?;
    let prepared = planner()?;
    let _payload_guards = pinned_handles(&prepared)?;
    // Revalidate hashes/profile/registry after read-only payload locks are held.
    if planner()? != prepared {
        return Err("Launch plan changed during final validation".into());
    }
    ensure_idle(&host.inventory()?)?;
    fs::create_dir_all(log_dir).map_err(error)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(error)?
        .as_nanos();
    let log = log_dir.join(format!("launch-{stamp}.log"));
    let mut child = host.dispatch(&prepared, &log)?;
    let launcher_pid = child.id();
    let start = Instant::now();
    let executable = path_key(&prepared.game_executable)?;
    loop {
        for process in host.inventory()? {
            if process.pid == launcher_pid
                && process
                    .executable
                    .as_deref()
                    .map(path_key)
                    .transpose()?
                    .as_ref()
                    == Some(&executable)
            {
                return Ok(LaunchResult {
                    launcher_pid,
                    game_pid: Some(process.pid),
                    log,
                    status: format!(
                        "Game dispatched (PID {launcher_pid}); fresh selected game observed (PID {}). Mod initialization is not yet verified.",
                        process.pid
                    ),
                });
            }
        }
        if let Some(code) = child.poll()? {
            return Err(format!(
                "Game dispatch exited with code {code}; no selected game was observed. See {}",
                log.display()
            ));
        }
        if start.elapsed() >= observation {
            return Ok(LaunchResult {
                launcher_pid,
                game_pid: None,
                log,
                status: format!(
                    "Game dispatched (PID {launcher_pid}); selected game startup is still unconfirmed. Mod initialization is not yet verified."
                ),
            });
        }
        host.pause();
    }
}

#[cfg(not(windows))]
fn game_processes() -> engine::Result<Vec<GameProcess>> {
    Err("Game launch is supported on Windows only".into())
}

#[cfg(windows)]
fn game_processes() -> engine::Result<Vec<GameProcess>> {
    use std::ffi::c_void;
    #[repr(C)]
    struct Entry {
        size: u32,
        usage: u32,
        pid: u32,
        heap: usize,
        module: u32,
        threads: u32,
        parent: u32,
        priority: i32,
        flags: u32,
        name: [u16; 260],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
        fn Process32FirstW(snapshot: *mut c_void, entry: *mut Entry) -> i32;
        fn Process32NextW(snapshot: *mut c_void, entry: *mut Entry) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn QueryFullProcessImageNameW(
            process: *mut c_void,
            flags: u32,
            name: *mut u16,
            length: *mut u32,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    let raw = unsafe { CreateToolhelp32Snapshot(2, 0) };
    if raw.is_null() || raw as isize == -1 {
        return Err(error(std::io::Error::last_os_error()));
    }
    let snapshot = Handle(raw);
    let mut entry: Entry = unsafe { std::mem::zeroed() };
    entry.size = std::mem::size_of::<Entry>() as u32;
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) };
    if more == 0 {
        return Err(error(std::io::Error::last_os_error()));
    }
    let mut games = Vec::new();
    while more != 0 {
        let end = entry
            .name
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.name.len());
        if String::from_utf16_lossy(&entry.name[..end]).eq_ignore_ascii_case("Paladins.exe") {
            let process = unsafe { OpenProcess(0x1000, 0, entry.pid) };
            let executable = if process.is_null() {
                None
            } else {
                let process = Handle(process);
                let mut buffer = vec![0u16; 32768];
                let mut length = buffer.len() as u32;
                if unsafe {
                    QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut length)
                } == 0
                {
                    None
                } else {
                    String::from_utf16(&buffer[..length as usize])
                        .ok()
                        .map(PathBuf::from)
                }
            };
            games.push(GameProcess {
                pid: entry.pid,
                executable,
            });
        }
        more = unsafe { Process32NextW(snapshot.0, &mut entry) };
    }
    let last = std::io::Error::last_os_error();
    if last.raw_os_error() != Some(18) {
        return Err(error(last));
    } // ERROR_NO_MORE_FILES
    Ok(games)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{
        cell::{Cell, RefCell},
        collections::VecDeque,
        sync::atomic::{AtomicU64, Ordering},
    };
    static SERIAL: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        home: PathBuf,
        options: Options,
    }
    impl Fixture {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let home = std::env::temp_dir().join(format!(
                "paladinscat-launch-fixture-{}-{stamp}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            let root = home.join("game");
            for dir in ["Binaries/Win64", "ChaosGame", "Engine"] {
                fs::create_dir_all(root.join(dir)).unwrap();
            }
            fs::write(root.join(GAME), b"fixture-game").unwrap();
            for relative in ENGINE_CONFIGS {
                let path = root.join(relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, b"[URL]\nfixture=true\n").unwrap();
            }
            Self {
                home,
                options: Options {
                    game_root: root,
                    language: "KOR".into(),
                    homedir: "paladinslive".into(),
                    platform: LaunchPlatform::default(),
                    performance: None,
                    rollback_input: false,
                },
            }
        }
        fn plan(&self, selected: &[PathBuf]) -> engine::Result<Plan> {
            plan_with(&self.options, |_, _| Ok(selected.to_vec()))
        }
        fn dll(&self, relative: &str) -> PathBuf {
            let path = self.options.game_root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"fixture-dll").unwrap();
            path.canonicalize().unwrap()
        }
        fn registry(&self, value: serde_json::Value) -> PathBuf {
            let path = self.options.game_root.join(REGISTRY);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            path
        }
        fn receipt_ime(&self) -> PathBuf {
            let source = self.home.join("source-ime.dll");
            fs::write(&source, ime_image()).unwrap();
            let package = ime::prepare(&source, &self.home.join("prepared-ime"), "0.1.0").unwrap();
            let dir = self.options.game_root.join(ime::TARGET);
            fs::create_dir_all(dir.join("dlls")).unwrap();
            fs::write(dir.join("manifest.toml"), "[mod]\nid = \"PaladinsCat-IME\"\nname = \"PaladinsCat IME Fix\"\nversion = \"0.1.0\"\n\n[[mod.authors]]\nname = \"PaladinsCat\"\n").unwrap();
            let dll = dir.join("dlls").join(ime::DLL);
            fs::copy(source, &dll).unwrap();
            let receipt = self
                .options
                .game_root
                .join(".watchcat-localization/ime-active.json");
            fs::create_dir_all(receipt.parent().unwrap()).unwrap();
            fs::write(receipt, serde_json::to_vec(&package).unwrap()).unwrap();
            assert_eq!(
                ime::installed_package(&self.options.game_root).unwrap(),
                Some(package)
            );
            dll.canonicalize().unwrap()
        }
        fn locale_receipt(&self) -> PathBuf {
            let target = self.dll("ChaosGame/fixture.dat");
            let backup = self.dll(".watchcat-localization/backups/fixture.dat");
            let sha = format!("{:x}", Sha256::digest(b"fixture-dll"));
            let receipt = self
                .options
                .game_root
                .join(".watchcat-localization/active.json");
            fs::write(&receipt, serde_json::to_vec(&serde_json::json!({
                "version":1,"locale":"KOR","game_root":self.options.game_root.canonicalize().unwrap(),"applied":true,
                "files":[{"target":target,"backup":backup,"stock_sha256":sha,"artifact_sha256":sha}]
            })).unwrap()).unwrap();
            receipt
        }
    }

    // Deliberately non-executable PE fixture: tests inspect the loader contract only.
    fn ime_image() -> Vec<u8> {
        let mut b = vec![0u8; 2048];
        b[..2].copy_from_slice(b"MZ");
        b[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
        b[64..68].copy_from_slice(b"PE\0\0");
        b[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
        b[70..72].copy_from_slice(&1u16.to_le_bytes());
        b[84..86].copy_from_slice(&240u16.to_le_bytes());
        b[86..88].copy_from_slice(&0x2000u16.to_le_bytes());
        b[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
        let set = |b: &mut [u8], at: usize, value: u32| {
            b[at..at + 4].copy_from_slice(&value.to_le_bytes())
        };
        for (at, value) in [
            (200, 4096),
            (204, 1024),
            (340, 4096),
            (344, 1536),
            (348, 512),
            (364, 0xe0000040),
            (532, 9),
            (536, 9),
            (540, 4184),
            (544, 4224),
            (548, 4264),
        ] {
            set(&mut b, at, value);
        }
        let names = [
            "WatchCatImeStatus",
            "WatchCatImeState",
            "WatchCatImeFlags",
            "WatchCatImeDisable",
            "WatchCatImePointers",
            "WatchCatImeAbi",
            "WatchCatImeWindow",
            "WatchCatImeWindowError",
            "WatchCatImeCounters",
        ];
        let mut at = 720;
        for (i, name) in names.iter().enumerate() {
            set(&mut b, 600 + i * 4, if i == 5 { 5120 } else { 5504 });
            set(&mut b, 640 + i * 4, 4096 + (at - 512) as u32);
            b[680 + i * 2..682 + i * 2].copy_from_slice(&(i as u16).to_le_bytes());
            b[at..at + name.len()].copy_from_slice(name.as_bytes());
            at += name.len() + 1;
        }
        for (i, value) in [4u64, 9, 7, 1].iter().enumerate() {
            b[1536 + i * 8..1544 + i * 8].copy_from_slice(&value.to_le_bytes());
        }
        b
    }

    #[test]
    fn external_paths_preserve_unicode_spaces_and_unc_without_device_syntax() {
        for (input, expected) in [
            (r"\\?\C:\Paladins\Season 8.1", r"C:\Paladins\Season 8.1"),
            (r"\\?\D:\번역 자료\input.dll", r"D:\번역 자료\input.dll"),
            (
                r"\\?\UNC\server\share\번역 자료",
                r"\\server\share\번역 자료",
            ),
            (r"C:\Paladins\Season 8.1", r"C:\Paladins\Season 8.1"),
        ] {
            assert_eq!(argument_path(Path::new(input)).unwrap(), expected);
        }
        for input in [
            r"\\?\GLOBALROOT\Device\HarddiskVolume1",
            r"\\.\C:",
            r"\\?\C:relative",
            r"\\?\UNC\server",
            r"\\?\UNC\\share",
        ] {
            assert!(argument_path(Path::new(input)).is_err(), "{input}");
        }
    }

    #[test]
    fn launch_command_resolves_relative_engine_config_from_game_binaries() {
        let f = Fixture::new();
        let dll = f.dll("cache/input with spaces.dll");
        let plan = f.plan(&[dll]).unwrap();
        let command = launch_command(&plan).unwrap();
        let cwd = command.get_current_dir().unwrap();
        assert_eq!(cwd.canonicalize().unwrap(), plan.working_directory);
        assert_eq!(
            cwd.join("../../ChaosGame/Config/DefaultEngine.ini")
                .canonicalize()
                .unwrap(),
            f.options
                .game_root
                .join(ENGINE_CONFIGS[1])
                .canonicalize()
                .unwrap()
        );
        assert_eq!(
            Path::new(command.get_program()).canonicalize().unwrap(),
            plan.game_executable
        );
        for value in std::iter::once(command.get_program())
            .chain(std::iter::once(cwd.as_os_str()))
            .chain(command.get_args())
        {
            assert!(!value.to_str().unwrap().contains(r"\\?\"));
        }
        assert_eq!(command.get_args().next().unwrap(), "-homedir=paladinslive");
        assert_eq!(plan.engine_configs.len(), 2);
        assert_eq!(pinned_handles(&plan).unwrap().len(), 4);
        assert_eq!(
            path_key(&f.options.game_root).unwrap(),
            path_key(&plan.game_root).unwrap()
        );
    }

    #[test]
    fn missing_or_empty_startup_config_refuses_dispatch() {
        let f = Fixture::new();
        let config = f.options.game_root.join(ENGINE_CONFIGS[1]);
        fs::write(&config, b"").unwrap();
        assert!(f.plan(&[]).unwrap_err().contains("missing or empty"));
        fs::rename(&config, config.with_extension("ini.saved")).unwrap();
        let host = fake(vec![vec![]]);
        let result = start_with(
            &f.options,
            &f.home.join("logs"),
            &host,
            || f.plan(&[]),
            Duration::ZERO,
        );
        assert!(
            result
                .unwrap_err()
                .contains("Required startup configuration")
        );
        assert_eq!(host.dispatched.get(), 0);
        assert!(!f.home.join("logs").exists());
    }

    #[cfg(windows)]
    #[test]
    fn native_module_relative_config_lookup_requires_ordinary_transport() {
        use std::{ffi::c_void, os::windows::ffi::OsStrExt};
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CreateFileW(
                name: *const u16,
                access: u32,
                share: u32,
                security: *mut c_void,
                creation: u32,
                flags: u32,
                template: *mut c_void,
            ) -> *mut c_void;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        fn native_open(path: &Path) -> std::io::Result<()> {
            let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            // Exercise the legacy read contract directly: std::fs can normalize
            // paths before CreateFileW and would hide this native failure.
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    0x8000_0000, // GENERIC_READ
                    1,           // FILE_SHARE_READ
                    std::ptr::null_mut(),
                    3, // OPEN_EXISTING
                    0x80,
                    std::ptr::null_mut(),
                )
            };
            if handle as isize == -1 {
                return Err(std::io::Error::last_os_error());
            }
            assert_ne!(unsafe { CloseHandle(handle) }, 0);
            Ok(())
        }
        let f = Fixture::new();
        let plan = f.plan(&[]).unwrap();
        let pins = pinned_handles(&plan).unwrap();
        let module_dir = plan.game_executable.parent().unwrap();
        let relative = r"..\..\ChaosGame\Config\DefaultEngine.ini";
        // FString concatenates raw text. PathBuf::join on a verbatim base can
        // normalize parent components and would mask the client behavior.
        let verbatim = PathBuf::from(format!(r"{}\{relative}", module_dir.display()));
        assert!(verbatim.to_str().unwrap().starts_with(r"\\?\"));
        let ordinary = PathBuf::from(format!(
            r"{}\{relative}",
            external_path(module_dir).unwrap()
        ));
        native_open(&ordinary).unwrap();
        let failure = native_open(&verbatim).unwrap_err();
        assert!(
            matches!(failure.raw_os_error(), Some(2 | 3 | 123)),
            "unexpected native path error: {failure}"
        );
        // Required-file pins coexist with the actual native shared reader.
        assert_eq!(pins.len(), 3);
    }

    #[cfg(windows)]
    #[test]
    fn ordinary_path_conversion_cannot_redirect_a_verbatim_filename() {
        let f = Fixture::new();
        let root = f.options.game_root.canonicalize().unwrap();
        let ordinary = root.join("ordinary.dll");
        let verbatim = root.join("ordinary.dll.");
        fs::write(&ordinary, b"ordinary").unwrap();
        fs::write(&verbatim, b"distinct-verbatim").unwrap();
        assert_ne!(fs::read(&ordinary).unwrap(), fs::read(&verbatim).unwrap());
        assert!(external_path(&verbatim).is_err());
        assert!(f.plan(&[verbatim]).is_err());
        assert!(external_path(&ordinary).is_ok());
    }

    #[test]
    fn standalone_plan_preserves_profile_and_separate_dll_load_order() {
        let mut f = Fixture::new();
        f.options.language = "FRA".into();
        f.options.homedir = "community-profile_2".into();
        let first = f.dll("cache/input.dll");
        let second = f.dll("cache/world.dll");
        let p = f.plan(&[first.clone(), second.clone()]).unwrap();
        assert_eq!(p.dlls, [first, second]);
        assert!(!p.args.contains(&"--dll".into()));
        assert!(!p.args.contains(&"--".into()));
        assert!(p.args.contains(&"-homedir=community-profile_2".into()));
        assert_eq!(p.args.last().unwrap(), "-Language=FRA");
        assert_eq!(
            p.game_executable,
            f.options.game_root.join(GAME).canonicalize().unwrap()
        );
        assert!(!f.options.game_root.join(".watchcat-localization").exists());
    }

    #[test]
    fn login_platforms_pass_exactly_one_tempest_flag_and_preserve_the_native_plan() {
        let mut f = Fixture::new();
        let dll = f.dll("cache/selected.dll");
        let pc = f.plan(std::slice::from_ref(&dll)).unwrap();
        for (platform, flag) in [
            (LaunchPlatform::Pc, None),
            (LaunchPlatform::Steam, Some("-steam")),
            (LaunchPlatform::Epic, Some("-epic")),
        ] {
            f.options.platform = platform;
            let mut expected = pc.clone();
            if let Some(flag) = flag {
                expected.args.insert(0, flag.into());
            }
            let actual = f.plan(std::slice::from_ref(&dll)).unwrap();
            assert_eq!(actual, expected);
            assert!(!actual.args.iter().any(|arg| arg == "-nosteam"));
            for other in ["-steam", "-epic"] {
                assert_eq!(
                    actual
                        .args
                        .iter()
                        .filter(|arg| arg.as_str() == other)
                        .count(),
                    usize::from(Some(other) == flag)
                );
            }
        }
        for invalid in ["", "-steam", "steam -epic", "epic\n", "unknown"] {
            assert!(LaunchPlatform::parse(invalid).is_err());
        }
    }

    #[test]
    fn stock_int_plan_allows_launch_without_localization_receipt() {
        let mut f = Fixture::new();
        f.options.language = "INT".into();
        let p = f.plan(&[]).unwrap();
        assert!(p.dlls.is_empty());
        assert_eq!(p.args.last().unwrap(), "-Language=INT");
        assert!(!f.options.game_root.join(".watchcat-localization").exists());
    }

    #[test]
    fn profile_injection_and_duplicate_dll_paths_are_rejected() {
        let mut f = Fixture::new();
        for locale in ["kor", "KOR -windowed", "INT\n", "KOR,"] {
            f.options.language = locale.into();
            assert!(f.plan(&[]).is_err());
        }
        f.options.language = "KOR".into();
        for home in ["", "../other", "-homedir=x", "safe\r"] {
            f.options.homedir = home.into();
            assert!(f.plan(&[]).is_err());
        }
        f.options.homedir = "paladinslive".into();
        let comma = f.dll("cache/comma,name.dll");
        assert_eq!(f.plan(std::slice::from_ref(&comma)).unwrap().dlls, [comma]);
        let unique = f.dll("cache/unique.dll");
        assert!(f.plan(&[unique.clone(), unique]).is_err());
        f.options.rollback_input = true;
        assert!(f.plan(&[]).is_err());
    }

    #[test]
    fn verified_unregistered_ime_uses_explicit_load_once() {
        let f = Fixture::new();
        let dll = f.receipt_ime();
        let p = f.plan(std::slice::from_ref(&dll)).unwrap();
        assert_eq!(p.dlls.as_slice(), std::slice::from_ref(&dll));
        assert!(!p.args.contains(&"--dll".into()));
        assert!(!f.options.game_root.join(REGISTRY).exists());
    }

    #[test]
    fn verified_registered_ime_omits_explicit_duplicate_and_preserves_registry() {
        let f = Fixture::new();
        let dll = f.receipt_ime();
        let registry = f.registry(serde_json::json!([{"Id":"PaladinsCat-IME","Name":"PaladinsCat IME Fix","Kind":"V2","Enabled":true}]));
        let original = fs::read(&registry).unwrap();
        let p = f.plan(std::slice::from_ref(&dll)).unwrap();
        assert_eq!(p.dlls, [dll]);
        assert!(!p.args.contains(&"--dll".into()));
        assert_eq!(fs::read(registry).unwrap(), original);
    }

    #[test]
    fn plain_mod_folders_and_prepared_cache_do_not_enable_features() {
        let f = Fixture::new();
        f.dll(".tempest/v2/mods/Unregistered/dlls/unregistered.dll");
        f.dll("cache/PaladinsCatImeFix.dll");
        let p = f.plan(&[]).unwrap();
        assert!(p.dlls.is_empty());
        assert!(!p.args.contains(&"--dll".into()));
        assert_eq!(ime::installed_package(&f.options.game_root).unwrap(), None);
        f.dll(&format!("{}/dlls/{}", ime::TARGET, ime::DLL));
        assert!(
            ime::installed_package(&f.options.game_root)
                .unwrap_err()
                .contains("without a recognized receipt")
        );
    }

    #[test]
    fn unselected_registered_native_family_and_duplicate_ids_are_refused() {
        let f = Fixture::new();
        f.dll(&format!("{}/dlls/{}", ime::TARGET, ime::DLL));
        f.registry(serde_json::json!([{"Id":"PaladinsCat-IME","Kind":"V2","Enabled":true}]));
        assert!(f.plan(&[]).unwrap_err().contains("Conflicting"));
        f.registry(serde_json::json!([{"Id":"Other","Kind":"V2","Enabled":true},{"Id":"oTHER","Kind":"V2","Enabled":false}]));
        assert!(f.plan(&[]).unwrap_err().contains("repeats an ID"));
        f.registry(serde_json::json!([{"Id":"paladins-performance","Kind":"V2","Enabled":true}]));
        assert!(
            f.plan(&[])
                .unwrap_err()
                .contains("Conflicting manually registered")
        );
    }

    #[test]
    fn enabled_manual_native_owner_is_refused_even_under_an_unrelated_id() {
        let f = Fixture::new();
        f.dll(".tempest/v2/mods/Other/dlls/watchcat-native-input-compact-modifier.dll");
        f.registry(serde_json::json!([{"Id":"Other","Kind":"V2","Enabled":true}]));
        assert!(
            f.plan(&[])
                .unwrap_err()
                .contains("Conflicting enabled native")
        );
        f.registry(serde_json::json!([{"Id":"Other","Kind":"V2","Enabled":false}]));
        assert!(f.plan(&[]).is_ok());
        f.registry(serde_json::json!([{"Id":"OldInput","Kind":"V1","Enabled":true,"OwnedFiles":[".tempest/v2/mods/Other/dlls/watchcat-native-input-compact-modifier.dll"]}]));
        assert!(
            f.plan(&[])
                .unwrap_err()
                .contains("Conflicting native DLL owner")
        );
    }

    #[test]
    fn imported_tempest_dll_enters_the_existing_plan_once_and_pending_recovery_blocks_play() {
        use std::io::Write;
        let f = Fixture::new();
        let package = f.home.join("fixture.tempest");
        let mut dll = vec![0u8; 128];
        dll[..2].copy_from_slice(b"MZ");
        dll[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
        dll[64..70].copy_from_slice(b"PE\0\0\x64\x86");
        dll[88..90].copy_from_slice(&[0x0b, 0x02]);
        let mut zip = zip::ZipWriter::new(fs::File::create(&package).unwrap());
        for (name, bytes) in [
            (
                "manifest.toml",
                b"[mod]\nid='fixture-native'\nname='Fixture native'\n".as_slice(),
            ),
            ("dlls/fixture.dll", dll.as_slice()),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        crate::mods::import_with(&f.options.game_root, &package, false, true, &|| false, None)
            .unwrap();
        let planned = f.plan(&[]).unwrap();
        assert_eq!(
            planned.dlls,
            vec![
                f.options
                    .game_root
                    .join(".tempest/v2/mods/fixture-native/dlls/fixture.dll")
                    .canonicalize()
                    .unwrap()
            ]
        );
        crate::mods::change_with(
            &f.options.game_root,
            "fixture-native",
            Some(false),
            &|| false,
            None,
        )
        .unwrap();
        assert!(f.plan(&[]).unwrap().dlls.is_empty());
        assert!(!crate::mods::list(&f.options.game_root).unwrap()[0].enabled);
        crate::mods::change_with(
            &f.options.game_root,
            "fixture-native",
            Some(true),
            &|| false,
            None,
        )
        .unwrap();
        assert_eq!(f.plan(&[]).unwrap().dlls.len(), 1);
        crate::mods::change_with(
            &f.options.game_root,
            "fixture-native",
            None,
            &|| false,
            None,
        )
        .unwrap();
        assert!(f.plan(&[]).unwrap().dlls.is_empty());
        assert!(crate::mods::list(&f.options.game_root).unwrap().is_empty());
        assert!(
            !f.options
                .game_root
                .join(".tempest/v2/mods/fixture-native")
                .exists()
        );
        let pending = f
            .options
            .game_root
            .join(".tempest/paladinscat-mod-transaction");
        fs::create_dir_all(&pending).unwrap();
        fs::write(pending.join("journal.json"), b"{}").unwrap();
        assert!(
            f.plan(&[])
                .unwrap_err()
                .contains("Recover the interrupted mod")
        );
    }

    #[test]
    fn enabled_unrelated_mods_are_preserved_and_unsafe_metadata_is_refused() {
        let f = Fixture::new();
        let dll = f.dll(".tempest/v2/mods/Independent/dlls/independent.dll");
        let registry =
            f.registry(serde_json::json!([{"Id":"Independent","Kind":"V2","Enabled":true}]));
        let original = fs::read(&registry).unwrap();
        assert_eq!(f.plan(&[]).unwrap().dlls, [dll]);
        assert_eq!(fs::read(&registry).unwrap(), original);
        fs::write(&registry, b"broken-json").unwrap();
        assert!(f.plan(&[]).is_err());
        f.registry(serde_json::json!([{"Id":"../escape","Kind":"V2","Enabled":true}]));
        assert!(f.plan(&[]).unwrap_err().contains("unsafe/missing ID"));
    }

    #[test]
    fn verified_korean_install_reports_language_selection_without_weakening_file_checks() {
        let f = Fixture::new();
        let receipt = f.locale_receipt();
        let before = fs::read(&receipt).unwrap();
        assert_eq!(
            verify_state(&f.options.game_root, "INT").unwrap_err(),
            LOCALIZATION_LANGUAGE_MISMATCH
        );
        assert!(verify_state(&f.options.game_root, "KOR").is_ok());
        assert_eq!(fs::read(&receipt).unwrap(), before);
        fs::write(
            f.options.game_root.join("ChaosGame/fixture.dat"),
            b"unexpected-change",
        )
        .unwrap();
        assert!(
            verify_state(&f.options.game_root, "INT")
                .unwrap_err()
                .contains("hash mismatch")
        );
    }

    #[test]
    fn incomplete_locale_receipt_and_pending_journals_block_play() {
        let f = Fixture::new();
        let receipt = f.locale_receipt();
        assert!(f.plan(&[]).is_ok());
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
        value["applied"] = false.into();
        fs::write(&receipt, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(f.plan(&[]).unwrap_err().contains("requires recovery"));
        value["applied"] = true.into();
        fs::write(&receipt, serde_json::to_vec(&value).unwrap()).unwrap();
        fs::write(
            f.options.game_root.join("ChaosGame/fixture.dat"),
            b"unexpected-change",
        )
        .unwrap();
        assert!(f.plan(&[]).unwrap_err().contains("hash mismatch"));
        fs::write(
            f.options
                .game_root
                .join(".watchcat-localization/ime-journal.json"),
            b"{}",
        )
        .unwrap();
        assert!(f.plan(&[]).unwrap_err().contains("Recover the interrupted"));
    }

    #[test]
    fn actual_plan_rejects_unsupported_game_and_does_not_spawn_or_write() {
        let f = Fixture::new();
        assert!(
            plan(&f.options)
                .unwrap_err()
                .contains("Supported game executable")
        );
        assert!(!f.options.game_root.join(".watchcat-localization").exists());
        assert!(!f.home.join("logs").exists());
    }

    struct FakeChild {
        code: Option<i32>,
    }
    impl Dispatch for FakeChild {
        fn id(&self) -> u32 {
            77
        }
        fn poll(&mut self) -> engine::Result<Option<i32>> {
            Ok(self.code)
        }
    }
    struct FakeHost {
        snapshots: RefCell<VecDeque<Vec<GameProcess>>>,
        dispatched: Cell<usize>,
        code: Option<i32>,
        inventory_error: bool,
    }
    impl Host for FakeHost {
        fn inventory(&self) -> engine::Result<Vec<GameProcess>> {
            if self.inventory_error {
                return Err("inventory unavailable".into());
            }
            let mut snapshots = self.snapshots.borrow_mut();
            if snapshots.len() > 1 {
                Ok(snapshots.pop_front().unwrap())
            } else {
                Ok(snapshots.front().cloned().unwrap_or_default())
            }
        }
        fn dispatch(&self, _: &Plan, _: &Path) -> engine::Result<Box<dyn Dispatch>> {
            self.dispatched.set(self.dispatched.get() + 1);
            Ok(Box::new(FakeChild { code: self.code }))
        }
        fn pause(&self) {}
    }
    fn fake(snapshots: Vec<Vec<GameProcess>>) -> FakeHost {
        FakeHost {
            snapshots: RefCell::new(snapshots.into()),
            dispatched: Cell::new(0),
            code: None,
            inventory_error: false,
        }
    }

    #[test]
    fn running_or_unqueryable_client_refuses_dispatch_without_killing_it() {
        let f = Fixture::new();
        let host = fake(vec![vec![GameProcess {
            pid: 42,
            executable: None,
        }]]);
        let error = start_with(
            &f.options,
            &f.home.join("logs"),
            &host,
            || f.plan(&[]),
            Duration::ZERO,
        )
        .unwrap_err();
        assert!(error.contains("42"));
        assert_eq!(host.dispatched.get(), 0);
        assert!(!f.options.game_root.join(".watchcat-localization").exists());
        let mut host = fake(vec![vec![]]);
        host.inventory_error = true;
        assert!(
            start_with(
                &f.options,
                &f.home.join("logs"),
                &host,
                || f.plan(&[]),
                Duration::ZERO
            )
            .is_err()
        );
        assert_eq!(host.dispatched.get(), 0);
    }

    #[test]
    fn client_appearing_during_preflight_refuses_dispatch() {
        let f = Fixture::new();
        let host = fake(vec![
            vec![],
            vec![],
            vec![GameProcess {
                pid: 43,
                executable: None,
            }],
        ]);
        assert!(
            start_with(
                &f.options,
                &f.home.join("logs"),
                &host,
                || f.plan(&[]),
                Duration::ZERO
            )
            .is_err()
        );
        assert_eq!(host.dispatched.get(), 0);
    }

    #[test]
    fn dispatch_observation_is_bounded_and_never_claims_hooks_active() {
        let f = Fixture::new();
        let host = fake(vec![vec![]]);
        let r = start_with(
            &f.options,
            &f.home.join("logs"),
            &host,
            || f.plan(&[]),
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(r.launcher_pid, 77);
        assert_eq!(r.game_pid, None);
        assert!(r.status.contains("startup is still unconfirmed"));
        assert!(r.status.contains("not yet verified"));
        assert_eq!(host.dispatched.get(), 1);
        let host = fake(vec![
            vec![],
            vec![],
            vec![],
            vec![GameProcess {
                pid: 77,
                executable: Some(f.options.game_root.join(GAME)),
            }],
        ]);
        let r = start_with(
            &f.options,
            &f.home.join("logs"),
            &host,
            || f.plan(&[]),
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(r.game_pid, Some(77));
        assert!(r.status.contains("not yet verified"));
    }

    #[test]
    fn executable_identity_resolves_existing_path_aliases() {
        let f = Fixture::new();
        let requested = f.options.game_root.join(GAME);
        let final_path = requested.canonicalize().unwrap();
        assert_eq!(
            path_key(&requested).unwrap(),
            path_key(&final_path).unwrap()
        );
        let dotted = f.options.game_root.join("Binaries/Win64/./Paladins.exe");
        assert_eq!(path_key(&dotted).unwrap(), path_key(&final_path).unwrap());
        assert!(path_key(&f.options.game_root.join("missing.exe")).is_err());
    }

    #[test]
    fn changed_final_plan_and_nonzero_dispatch_exit_are_distinct_failures() {
        let f = Fixture::new();
        let host = fake(vec![vec![]]);
        let calls = Cell::new(0);
        let result = start_with(
            &f.options,
            &f.home.join("logs"),
            &host,
            || {
                let mut p = f.plan(&[])?;
                calls.set(calls.get() + 1);
                if calls.get() > 1 {
                    p.args.push("unexpected".into());
                }
                Ok(p)
            },
            Duration::ZERO,
        );
        assert!(
            result
                .unwrap_err()
                .contains("changed during final validation")
        );
        assert_eq!(host.dispatched.get(), 0);
        let mut host = fake(vec![vec![]]);
        host.code = Some(7);
        assert!(
            start_with(
                &f.options,
                &f.home.join("logs"),
                &host,
                || f.plan(&[]),
                Duration::ZERO
            )
            .unwrap_err()
            .contains("exited with code 7")
        );
    }

    #[test]
    fn automatic_dll_bytes_are_pinned_and_foreign_same_path_pid_is_not_ours() {
        let f = Fixture::new();
        let dll = f.dll(".tempest/v2/mods/Independent/dlls/independent.dll");
        f.registry(serde_json::json!([{"Id":"Independent","Kind":"V2","Enabled":true}]));
        let first = f.plan(&[]).unwrap();
        fs::write(&dll, b"changed-before-pinning").unwrap();
        let changed = f.plan(&[]).unwrap();
        assert_ne!(first, changed);
        assert_eq!(first.dlls, changed.dlls);
        let pins = pinned_handles(&changed).unwrap();
        #[cfg(windows)]
        assert!(fs::write(&dll, b"change-while-pinned").is_err());
        drop(pins);
        let host = fake(vec![
            vec![],
            vec![],
            vec![],
            vec![GameProcess {
                pid: 88,
                executable: Some(f.options.game_root.join(GAME)),
            }],
        ]);
        let result = start_with(
            &f.options,
            &f.home.join("logs"),
            &host,
            || f.plan(&[]),
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(result.launcher_pid, 77);
        assert_eq!(result.game_pid, None);
    }
}
