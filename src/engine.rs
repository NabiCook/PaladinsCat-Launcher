use crate::upk_patcher;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub type Result<T> = std::result::Result<T, String>;
pub type Progress<'a> = &'a dyn Fn(String);

/// Product labels contain only the numeric package revision. Descriptive legacy
/// IDs and content hashes remain intact for cache/receipt ownership and diagnostics.
pub fn package_version_label(bundle_id: &str) -> String {
    for part in bundle_id.split('-') {
        if let Some(digits) = part.strip_prefix('v')
            && !digits.is_empty()
            && digits.bytes().all(|c| c.is_ascii_digit())
        {
            return format!("v{digits}");
        }
    }
    String::new()
}

pub fn package_caption(bundle_id: &str, locale: &str, build: &str) -> String {
    let version = package_version_label(bundle_id);
    format!(
        "{} · {build}",
        if version.is_empty() { locale } else { &version }
    )
}

pub fn installed_package_caption(status: &str) -> String {
    if let Some((locale, identity)) = status.split_once(" · ") {
        let version = package_version_label(identity);
        if !version.is_empty() {
            return format!("{locale} · {version}");
        }
    }
    status.to_owned()
}
pub fn save_settings<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    upk_patcher::save_json_atomic(path, value)
}

#[cfg(test)]
mod version_label_tests {
    use super::*;
    #[test]
    fn product_versions_hide_descriptions_and_hashes_without_changing_identity() {
        for identity in [
            "kor-ui-v177-gold-1a73a309d6ab",
            "kor-ui-v177-1a73a309d6ab",
            "kor-ui-v177-approved",
            "kor-ui-v177",
        ] {
            assert_eq!(package_caption(identity, "KOR", "8.1"), "v177 · 8.1");
        }
        assert_eq!(package_caption("legacy-package", "KOR", "8.1"), "KOR · 8.1");
        assert_eq!(package_version_label("kor-ui-version177-gold"), "");
        assert_eq!(
            installed_package_caption("KOR · kor-ui-v177-gold-1a73a309d6ab"),
            "KOR · v177"
        );
        assert_eq!(installed_package_caption("Not installed"), "Not installed");
    }
}
const LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const STATE: &str = ".watchcat-localization";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Package {
    pub version: u32,
    pub bundle_id: String,
    pub locale: String,
    #[serde(default = "local_build")]
    pub game_build: String,
    #[serde(default = "platform")]
    pub platform: String,
    #[serde(default)]
    pub source_commit: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub min_tool_version: Option<[u32; 3]>,
    #[serde(default)]
    pub game_fingerprint: Option<Fingerprint>,
    pub files: Vec<Payload>,
}
fn local_build() -> String {
    "Local package · compatibility checked by file hashes".into()
}
fn platform() -> String {
    "Win64".into()
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub path: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Payload {
    pub source: String,
    pub target: String,
    pub stock_sha256: String,
    pub artifact_sha256: String,
    #[serde(default)]
    pub artifact_bytes: Option<u64>,
    #[serde(default)]
    pub download: Option<Download>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Download {
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
    #[serde(default)]
    pub encoding: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Release {
    pub bundle_id: String,
    pub locale: String,
    pub game_build: String,
    pub manifest_url: String,
    pub manifest_sha256: String,
    #[serde(default)]
    pub notes: String,
}
#[derive(Deserialize)]
pub struct Catalog {
    pub schema_version: u32,
    pub packages: Vec<Release>,
    #[serde(default)]
    pub ime_packages: Vec<crate::ime::Release>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cached {
    pub package: Package,
    pub directory: PathBuf,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    previous: Option<Cached>,
    next: Option<Cached>,
}
#[derive(Deserialize)]
struct Active {
    version: u32,
    bundle_id: String,
    locale: String,
    game_root: PathBuf,
    applied: bool,
    files: Vec<ActiveFile>,
}
#[derive(Deserialize)]
struct ActiveFile {
    target: PathBuf,
    backup: PathBuf,
    stock_sha256: String,
    artifact_sha256: String,
}

pub fn data_dir() -> PathBuf {
    local_app_data().join("PaladinsCat").join("Localization")
}
#[cfg(windows)]
fn local_app_data() -> PathBuf {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }
    const LOCAL_APP_DATA: Guid = Guid {
        data1: 0xf1b32785,
        data2: 0x6fba,
        data3: 0x4fcf,
        data4: [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91],
    };
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHGetKnownFolderPath(
            folder: *const Guid,
            flags: u32,
            token: *mut std::ffi::c_void,
            path: *mut *mut u16,
        ) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(pointer: *mut std::ffi::c_void);
    }
    let mut pointer = std::ptr::null_mut();
    // Packaged hosts can redirect LOCALAPPDATA. Keep the existing user's settings.
    let result = unsafe {
        SHGetKnownFolderPath(
            &LOCAL_APP_DATA,
            0x0001_0000, // KF_FLAG_NO_PACKAGE_REDIRECTION
            std::ptr::null_mut(),
            &mut pointer,
        )
    };
    if result >= 0 && !pointer.is_null() {
        let mut length = 0;
        // SHGetKnownFolderPath returns a null-terminated, task-allocated string.
        unsafe {
            while *pointer.add(length) != 0 {
                length += 1;
            }
            let path = PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(
                pointer, length,
            )));
            CoTaskMemFree(pointer.cast());
            return path;
        }
    }
    if !pointer.is_null() {
        unsafe { CoTaskMemFree(pointer.cast()) };
    }
    std::env::var_os("USERPROFILE")
        .map(|profile| PathBuf::from(profile).join("AppData").join("Local"))
        .unwrap_or_else(std::env::temp_dir)
}
#[cfg(not(windows))]
fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
}
fn hash(value: &str) -> Result<()> {
    if value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("Invalid SHA-256 in package".into())
    }
}
fn relative(value: &str) -> Result<()> {
    if value.is_empty()
        || value.contains([':', '\\'])
        || Path::new(value)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        Err("Unsafe package path".into())
    } else {
        Ok(())
    }
}
fn contained(root: &Path, value: &str) -> Result<PathBuf> {
    relative(value)?;
    let result = root.join(value).canonicalize().map_err(err)?;
    if result.starts_with(root) {
        Ok(result)
    } else {
        Err("Path resolves outside selected folder".into())
    }
}
fn json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let file = File::open(path).map_err(err)?;
    let mut bytes = Vec::new();
    file.take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("Manifest exceeds size limit".into());
    }
    serde_json::from_slice(&bytes).map_err(err)
}
fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    upk_patcher::save_json_atomic(path, value)
}
fn tool_version() -> [u32; 3] {
    [
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH"),
    ]
    .map(|part| {
        part.parse()
            .expect("Cargo supplies numeric version components")
    })
}
fn validate(package: &Package) -> Result<()> {
    if package
        .min_tool_version
        .is_some_and(|version| version > tool_version())
    {
        return Err("This package requires a newer PaladinsCat Launcher".into());
    }
    if package.version != 1
        || !id(&package.bundle_id)
        || !id(&package.locale)
        || package.platform != "Win64"
        || package.files.is_empty()
        || package.files.len() > 32
    {
        return Err("Unsupported package schema, locale, platform or file count".into());
    }
    let mut targets = HashSet::new();
    let mut sources = HashSet::new();
    for file in &package.files {
        relative(&file.source)?;
        relative(&file.target)?;
        if Path::new(&file.source).components().count() != 1 {
            return Err("Artifact source must be a filename".into());
        }
        let target = file.target.to_ascii_lowercase();
        let locale_root = format!(
            "chaosgame/localization/{}/",
            package.locale.to_ascii_lowercase()
        );
        let allowed = target.starts_with(&locale_root)
            || ["gfxfonts.upk", "lobbyui_sf.upk", "gameui_sf.upk"]
                .iter()
                .any(|name| target == format!("chaosgame/cookedpcconsole/{name}"));
        if !allowed || !targets.insert(target) || !sources.insert(file.source.to_ascii_lowercase())
        {
            return Err("Package target is unsupported or repeated".into());
        }
        hash(&file.stock_sha256)?;
        hash(&file.artifact_sha256)?;
        if file.artifact_bytes.is_some_and(|n| n > LIMIT) {
            return Err("Artifact exceeds size limit".into());
        }
        if let Some(download) = &file.download {
            hash(&download.sha256)?;
            trusted_url(&download.url)?;
            if download.bytes > LIMIT || !matches!(download.encoding.as_str(), "" | "raw" | "gzip")
            {
                return Err("Unsupported download encoding/size".into());
            }
        }
    }
    Ok(())
}
pub(crate) fn trusted_url(value: &str) -> Result<()> {
    #[cfg(test)]
    if value.starts_with("http://127.0.0.1:") {
        return Ok(());
    }
    // Public catalogs/assets need no token, credential injection or custom TLS bypass.
    if (value.starts_with("https://raw.githubusercontent.com/")
        || value.starts_with("https://github.com/"))
        && !value.contains(['@', '#'])
    {
        Ok(())
    } else {
        Err("Use a public HTTPS GitHub catalog/release asset URL".into())
    }
}
pub(crate) fn fetch(
    url: &str,
    path: &Path,
    expected_hash: &str,
    expected_bytes: Option<u64>,
    progress: Progress<'_>,
) -> Result<()> {
    trusted_url(url)?;
    hash(expected_hash)?;
    if path.exists() && upk_patcher::require_hash(path, expected_hash, "cached download").is_ok() {
        return Ok(());
    }
    let parent = path.parent().ok_or("Cache path missing parent")?;
    fs::create_dir_all(parent).map_err(err)?;
    let part = path.with_extension(format!("part-{}", stamp()));
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .build();
    let response = agent
        .get(url)
        .set("Accept-Encoding", "identity")
        .call()
        .map_err(err)?;
    let mut input = response.into_reader();
    let mut output = File::create(&part).map_err(err)?;
    let mut buffer = [0; 128 * 1024];
    let mut total = 0;
    let limit = expected_bytes.unwrap_or(4 * 1024 * 1024);
    loop {
        let count = input.read(&mut buffer).map_err(err)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > limit {
            return Err("Download exceeds declared size".into());
        }
        output.write_all(&buffer[..count]).map_err(err)?;
        if total % (4 * 1024 * 1024) < buffer.len() as u64 {
            progress(format!(
                "Downloading {:.1} / {:.1} MiB",
                total as f64 / 1048576.,
                limit as f64 / 1048576.
            ));
        }
    }
    output.sync_all().map_err(err)?;
    drop(output);
    if expected_bytes.is_some_and(|bytes| bytes != total) {
        return Err("Download size mismatch".into());
    }
    upk_patcher::require_hash(&part, expected_hash, "download")?;
    if path.exists() {
        fs::rename(path, path.with_extension(format!("quarantine-{}", stamp()))).map_err(err)?;
    }
    fs::rename(part, path).map_err(err)
}
pub fn check_updates(url: &str) -> Result<Vec<Release>> {
    Ok(check_catalog(url)?.packages)
}
pub fn check_catalog(url: &str) -> Result<Catalog> {
    trusted_url(url)?;
    let response = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .build()
        .get(url)
        .call()
        .map_err(err)?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("Catalog exceeds size limit".into());
    }
    parse_catalog(&bytes)
}
pub fn parse_catalog(bytes: &[u8]) -> Result<Catalog> {
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("Catalog exceeds size limit".into());
    }
    let catalog: Catalog = serde_json::from_slice(bytes).map_err(err)?;
    if catalog.schema_version != 1 || catalog.packages.len() + catalog.ime_packages.len() > 256 {
        return Err("Unsupported catalog".into());
    }
    for release in &catalog.packages {
        if !id(&release.bundle_id) || !id(&release.locale) {
            return Err("Invalid release identity".into());
        }
        trusted_url(&release.manifest_url)?;
        hash(&release.manifest_sha256)?;
    }
    for release in &catalog.ime_packages {
        crate::ime::validate_release(release)?;
    }
    Ok(catalog)
}
pub fn download(release: &Release, cache: &Path, progress: Progress<'_>) -> Result<Cached> {
    let _lease = crate::cache::lock(cache)?;
    crate::cache::no_pending_transactions(cache)?;
    let mut attempt =
        crate::cache::Attempt::begin(cache, &["artifacts", "bundles", "downloads", "manifests"])?;
    hash(&release.manifest_sha256)?;
    if !id(&release.bundle_id) || !id(&release.locale) {
        return Err("Invalid release identity".into());
    }
    let manifest = cache.join("manifests").join(&release.manifest_sha256);
    progress("Downloading release manifest…".into());
    fetch(
        &release.manifest_url,
        &manifest,
        &release.manifest_sha256,
        None,
        progress,
    )?;
    let package: Package = json(&manifest)?;
    validate(&package)?;
    if package.bundle_id != release.bundle_id
        || package.locale != release.locale
        || package.game_build != release.game_build
    {
        return Err("Catalog and manifest identity differ".into());
    }
    if package.source_commit.len() != 40
        || !package.source_commit.bytes().all(|c| c.is_ascii_hexdigit())
        || package.game_fingerprint.is_none()
    {
        return Err("Published package requires source commit and game fingerprint".into());
    }
    for file in &package.files {
        let destination = cache.join("artifacts").join(&file.artifact_sha256);
        if destination.exists()
            && upk_patcher::require_hash(&destination, &file.artifact_sha256, "cached artifact")
                .is_ok()
        {
            continue;
        }
        let remote = file
            .download
            .as_ref()
            .ok_or("Remote artifact has no download descriptor")?;
        progress(format!("Preparing {}", file.source));
        let compressed = cache.join("downloads").join(&remote.sha256);
        fetch(
            &remote.url,
            &compressed,
            &remote.sha256,
            Some(remote.bytes),
            progress,
        )?;
        fs::create_dir_all(destination.parent().unwrap()).map_err(err)?;
        let part = destination.with_extension(format!("decoded-{}", stamp()));
        let input = File::open(compressed).map_err(err)?;
        let mut input: Box<dyn Read> = if remote.encoding == "gzip" {
            Box::new(flate2::read::GzDecoder::new(input))
        } else {
            Box::new(input)
        };
        let limit = file
            .artifact_bytes
            .ok_or("Remote artifact has no decoded size")?;
        let mut output = File::create(&part).map_err(err)?;
        let count = std::io::copy(&mut input.by_ref().take(limit + 1), &mut output).map_err(err)?;
        output.sync_all().map_err(err)?;
        drop(output);
        if count != limit {
            return Err("Decoded artifact size mismatch".into());
        }
        upk_patcher::require_hash(&part, &file.artifact_sha256, "decoded artifact")?;
        if destination.exists() {
            fs::rename(
                &destination,
                destination.with_extension(format!("quarantine-{}", stamp())),
            )
            .map_err(err)?;
        }
        fs::rename(part, destination).map_err(err)?;
    }
    let cached = materialize(package, cache)?;
    attempt.commit();
    crate::cache::prepared_cleanup(crate::cache::retain_translations(
        cache,
        &[&cached],
        Some(&release.manifest_sha256),
    ));
    Ok(cached)
}
pub fn import(path: &Path, cache: &Path, progress: Progress<'_>) -> Result<Cached> {
    let _lease = crate::cache::lock(cache)?;
    crate::cache::no_pending_transactions(cache)?;
    let mut attempt =
        crate::cache::Attempt::begin(cache, &["artifacts", "bundles", "downloads", "manifests"])?;
    let package: Package = json(path)?;
    validate(&package)?;
    let root = path
        .parent()
        .ok_or("Manifest parent missing")?
        .canonicalize()
        .map_err(err)?;
    for file in &package.files {
        progress(format!("Verifying {}", file.source));
        let source = contained(&root, &file.source)?;
        upk_patcher::require_hash(&source, &file.artifact_sha256, "imported artifact")?;
        cache_artifact(
            &source,
            &cache.join("artifacts").join(&file.artifact_sha256),
            &file.artifact_sha256,
        )?;
    }
    let cached = materialize(package, cache)?;
    attempt.commit();
    crate::cache::prepared_cleanup(crate::cache::retain_translations(cache, &[&cached], None));
    Ok(cached)
}
fn cache_artifact(source: &Path, destination: &Path, expected: &str) -> Result<()> {
    if destination.exists() {
        return upk_patcher::require_hash(destination, expected, "cached artifact");
    }
    fs::create_dir_all(destination.parent().ok_or("Cache parent absent")?).map_err(err)?;
    let part = destination.with_extension(format!("staged-{}", stamp()));
    fs::copy(source, &part).map_err(err)?;
    OpenOptions::new()
        .write(true)
        .open(&part)
        .and_then(|f| f.sync_all())
        .map_err(err)?;
    upk_patcher::require_hash(&part, expected, "staged artifact")?;
    fs::rename(part, destination).map_err(err)
}
fn materialize(package: Package, cache: &Path) -> Result<Cached> {
    let encoded = serde_json::to_vec(&package).map_err(err)?;
    use sha2::{Digest, Sha256};
    let manifest_hash = format!("{:x}", Sha256::digest(&encoded));
    let directory = cache.join("bundles").join(manifest_hash);
    fs::create_dir_all(&directory).map_err(err)?;
    for file in &package.files {
        cache_artifact(
            &cache.join("artifacts").join(&file.artifact_sha256),
            &directory.join(&file.source),
            &file.artifact_sha256,
        )?;
    }
    save(&directory.join("bundle.json"), &package)?;
    Ok(Cached { package, directory })
}
fn verify_cached(cached: &Cached) -> Result<()> {
    validate(&cached.package)?;
    let stored: Package = json(&cached.directory.join("bundle.json"))?;
    if serde_json::to_vec(&stored).map_err(err)?
        != serde_json::to_vec(&cached.package).map_err(err)?
    {
        return Err("Cached manifest was changed".into());
    }
    let root = cached.directory.canonicalize().map_err(err)?;
    for file in &cached.package.files {
        upk_patcher::require_hash(
            &contained(&root, &file.source)?,
            &file.artifact_sha256,
            "prepared artifact",
        )?;
    }
    Ok(())
}
pub fn installed(root: &Path) -> Result<String> {
    let path = root.join(STATE).join("active.json");
    if !path.exists() {
        return Ok("Original game files".into());
    }
    let state: Active = json(&path)?;
    Ok(if state.applied {
        format!("{} · {}", state.locale, state.bundle_id)
    } else {
        "Recovery required · interrupted installation".into()
    })
}
pub fn recovery_pending(root: &Path) -> bool {
    root.join(STATE).join("patcher-journal.json").exists()
}
fn snapshot(root: &Path, _cache: &Path) -> Result<Option<Cached>> {
    let path = root.join(STATE).join("active.json");
    if !path.exists() {
        return Ok(None);
    }
    let active: Active = json(&path)?;
    if active.version != 1 || active.game_root != root || !active.applied || !id(&active.bundle_id)
    {
        return Err("Installed receipt requires recovery".into());
    }
    let mut files = Vec::new();
    for (i, file) in active.files.iter().enumerate() {
        if !file.target.canonicalize().map_err(err)?.starts_with(root)
            || !file
                .backup
                .canonicalize()
                .map_err(err)?
                .starts_with(root.join(STATE))
        {
            return Err("Installed receipt path is outside the game".into());
        }
        upk_patcher::require_hash(&file.backup, &file.stock_sha256, "original backup")?;
        upk_patcher::require_hash(&file.target, &file.artifact_sha256, "installed artifact")?;
        let relative = file
            .target
            .strip_prefix(root)
            .map_err(err)?
            .to_string_lossy()
            .replace('\\', "/");
        files.push(Payload {
            source: format!("previous-{i}.bin"),
            target: relative,
            stock_sha256: file.stock_sha256.clone(),
            artifact_sha256: file.artifact_sha256.clone(),
            artifact_bytes: Some(fs::metadata(&file.target).map_err(err)?.len()),
            download: None,
        });
    }
    let mut previous = Package {
        version: 1,
        bundle_id: active.bundle_id,
        locale: active.locale,
        game_build: local_build(),
        platform: platform(),
        source_commit: String::new(),
        notes: "Verified snapshot of the previous installed package".into(),
        min_tool_version: None,
        game_fingerprint: None,
        files,
    };
    validate(&previous)?;
    let selected_path = root.join(STATE).join("selected-package.json");
    if selected_path.exists() {
        let selected: Option<Cached> = json(&selected_path)?;
        if let Some(selected) = selected {
            let same = selected.package.bundle_id == previous.bundle_id
                && selected.package.locale == previous.locale
                && selected.package.files.len() == previous.files.len()
                && previous.files.iter().all(|file| {
                    selected.package.files.iter().any(|old| {
                        old.target.eq_ignore_ascii_case(&file.target)
                            && old.stock_sha256.eq_ignore_ascii_case(&file.stock_sha256)
                            && old
                                .artifact_sha256
                                .eq_ignore_ascii_case(&file.artifact_sha256)
                    })
                });
            if !same {
                return Err("Selected package differs from installed receipt".into());
            }
            // Selection metadata remains useful when old downloads were cleared.
            // Rebuild its bytes only from the independently verified live receipt.
            validate(&selected.package)?;
            previous = selected.package;
        }
    }
    let recovery = root.join(STATE).join("recovery-cache");
    let _recovery_lease = crate::cache::lock(&recovery)?;
    for file in &previous.files {
        cache_artifact(
            &contained(root, &file.target)?,
            &recovery.join("artifacts").join(&file.artifact_sha256),
            &file.artifact_sha256,
        )?;
    }
    let cached = materialize(previous, &recovery)?;
    prune_recovery(root, Some(&cached))?;
    Ok(Some(cached))
}

fn prune_recovery(root: &Path, checkpoint: Option<&Cached>) -> Result<()> {
    let recovery = root.join(STATE).join("recovery-cache");
    if !recovery.exists() {
        return Ok(());
    }
    let recovery = recovery.canonicalize().map_err(err)?;
    let mut retained = Vec::new();
    if let Some(checkpoint) = checkpoint {
        retained.push(checkpoint.clone());
    }
    let rollback_path = root.join(STATE).join("rollback-package.json");
    if rollback_path.exists() {
        let rollback: Option<Cached> = json(&rollback_path)?;
        if let Some(rollback) = rollback
            && rollback.directory.starts_with(&recovery)
            && retained.iter().all(|p| p.directory != rollback.directory)
        {
            verify_cached(&rollback)?;
            retained.push(rollback);
        }
    }
    crate::cache::retain_translations(&recovery, &retained.iter().collect::<Vec<_>>(), None)
}

#[cfg(test)]
mod tests;
fn preflight(root: &Path, next: &Cached, previous: Option<&Cached>) -> Result<()> {
    verify_cached(next)?;
    if let Some(fingerprint) = &next.package.game_fingerprint {
        hash(&fingerprint.sha256)?;
        upk_patcher::require_hash(
            &contained(root, &fingerprint.path)?,
            &fingerprint.sha256,
            "game build",
        )?;
    }
    let mut needed = 0;
    for file in &next.package.files {
        let target = contained(root, &file.target)?;
        let old = previous.and_then(|old| {
            old.package
                .files
                .iter()
                .find(|old| old.target.eq_ignore_ascii_case(&file.target))
        });
        if old.is_some_and(|old| !old.stock_sha256.eq_ignore_ascii_case(&file.stock_sha256)) {
            return Err("Package belongs to a different game build".into());
        }
        upk_patcher::require_hash(
            &target,
            old.map_or(&file.stock_sha256, |old| &old.artifact_sha256),
            "current game file",
        )?;
        needed += fs::metadata(next.directory.join(&file.source))
            .map_err(err)?
            .len()
            + fs::metadata(target).map_err(err)?.len();
    }
    #[cfg(windows)]
    if available_bytes(root)? < needed + 16 * 1024 * 1024 {
        return Err("Insufficient disk space for staging and backups".into());
    }
    Ok(())
}
#[cfg(windows)]
fn available_bytes(root: &Path) -> Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(
            path: *const u16,
            available: *mut u64,
            total: *mut u64,
            free: *mut u64,
        ) -> i32;
    }
    let path: Vec<_> = root.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0;
    if unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        Err(err(std::io::Error::last_os_error()))
    } else {
        Ok(available)
    }
}
pub(crate) fn acquire(root: &Path, running: &dyn Fn() -> bool) -> Result<(PathBuf, File)> {
    let acquired = acquire_for_mod_recovery(root, running)?;
    if crate::mods::recovery_pending(&acquired.0) {
        return Err("Recover the interrupted mod import first".into());
    }
    Ok(acquired)
}

pub(crate) fn acquire_for_mod_recovery(
    root: &Path,
    running: &dyn Fn() -> bool,
) -> Result<(PathBuf, File)> {
    if running() {
        return Err("Close Paladins before changing game files".into());
    }
    let root = root.canonicalize().map_err(err)?;
    if !root.join("ChaosGame").is_dir() {
        return Err("Select the game folder containing ChaosGame".into());
    }
    let state = root.join(STATE);
    if state.exists() && !state.canonicalize().map_err(err)?.starts_with(&root) {
        return Err("Game state folder escapes installation".into());
    }
    fs::create_dir_all(&state).map_err(err)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(state.join("patcher.lock"))
        .map_err(err)?;
    lock.try_lock()
        .map_err(|_| "Another patch operation is using this installation".to_string())?;
    Ok((root, lock))
}
fn finish(root: &Path, selected: &Option<Cached>) -> Result<Option<String>> {
    save(&root.join(STATE).join("selected-package.json"), selected)?;
    fs::rename(
        root.join(STATE).join("patcher-journal.json"),
        root.join(STATE).join(format!("completed-{}.json", stamp())),
    )
    .map_err(err)?;
    // Keep only the one previous-version checkpoint; completed journals don't pin data.
    let checkpoint = selected.as_ref().filter(|cached| {
        cached
            .directory
            .starts_with(root.join(STATE).join("recovery-cache"))
    });
    // Once the journal is completed, recycling is maintenance, not install failure.
    Ok(prune_recovery(root, checkpoint).err())
}
fn recover_inner(
    root: &Path,
    running: &dyn Fn() -> bool,
    progress: Progress<'_>,
) -> Result<Option<String>> {
    let journal: Journal = json(&root.join(STATE).join("patcher-journal.json"))?;
    if let Some(previous) = &journal.previous {
        verify_cached(previous)?;
        if let Some(fingerprint) = &previous.package.game_fingerprint {
            hash(&fingerprint.sha256)?;
            upk_patcher::require_hash(
                &contained(root, &fingerprint.path)?,
                &fingerprint.sha256,
                "game build",
            )?;
        }
    }
    if running() {
        return Err("Close Paladins before recovery".into());
    }
    progress("Recovering the previous verified installation…".into());
    upk_patcher::revert_bundle(root.to_str().ok_or("Invalid game folder")?)?;
    if let Some(previous) = &journal.previous {
        upk_patcher::apply_bundle(
            &previous.directory,
            &previous.package.locale,
            root.to_str().unwrap(),
        )?;
    }
    finish(root, &journal.previous)
}
#[derive(Clone, Copy)]
pub enum Action {
    Apply,
    Rollback,
    Restore,
    Recover,
}
pub fn operate(
    action: Action,
    game: &Path,
    selected: Option<Cached>,
    cache: &Path,
    progress: Progress<'_>,
) -> Result<String> {
    transition(
        action,
        game,
        selected,
        cache,
        &upk_patcher::is_game_running,
        progress,
    )
}
fn transition(
    action: Action,
    game: &Path,
    selected: Option<Cached>,
    cache: &Path,
    running: &dyn Fn() -> bool,
    progress: Progress<'_>,
) -> Result<String> {
    let _cache_lease = crate::cache::lock(cache)?;
    let (root, _lock) = acquire(game, running)?;
    crate::cache::register_game(cache, &root)?;
    if matches!(action, Action::Recover) {
        let warning = recover_inner(&root, running, progress)?;
        return Ok(with_cleanup_warning(
            "Recovered the previous installation".into(),
            warning,
        ));
    }
    if recovery_pending(&root) {
        return Err("Recover the interrupted operation first".into());
    }
    let previous = snapshot(&root, cache)?;
    let next = match action {
        Action::Apply => Some(selected.ok_or("Choose/download/import a package first")?),
        Action::Rollback => json(&root.join(STATE).join("rollback-package.json"))?,
        Action::Restore => None,
        Action::Recover => unreachable!(),
    };
    if let Some(next) = &next {
        preflight(&root, next, previous.as_ref())?;
    }
    if running() {
        return Err("Game started during preparation; nothing applied".into());
    }
    save(
        &root.join(STATE).join("patcher-journal.json"),
        &Journal {
            previous: previous.clone(),
            next: next.clone(),
        },
    )?;
    let result = (|| {
        progress("Restoring verified originals before switching packages…".into());
        upk_patcher::revert_bundle(root.to_str().unwrap())?;
        if running() {
            return Err("Game started; close it before recovery".into());
        }
        if let Some(next) = &next {
            progress(format!(
                "Installing and verifying {}…",
                next.package.bundle_id
            ));
            upk_patcher::apply_bundle(
                &next.directory,
                &next.package.locale,
                root.to_str().unwrap(),
            )?;
        }
        save(&root.join(STATE).join("rollback-package.json"), &previous)?;
        finish(&root, &next)
    })();
    let warning = match result {
        Ok(warning) => warning,
        Err(error) => {
            return Err(match recover_inner(&root, running, progress) {
                Ok(warning) => with_cleanup_warning(
                    format!("Operation failed; previous installation restored: {error}"),
                    warning,
                ),
                Err(recovery) => format!("{error}. Recovery required: {recovery}"),
            });
        }
    };
    let message = if let Some(next) = next {
        format!("Applied {} · all files verified", next.package.bundle_id)
    } else {
        "Original game files restored and verified".into()
    };
    Ok(with_cleanup_warning(message, warning))
}

fn with_cleanup_warning(message: String, warning: Option<String>) -> String {
    warning.map_or_else(
        || message.clone(),
        |error| format!("{message} · recovery cache cleanup pending: {error}"),
    )
}
