//! Bounded package caches. Installed files and recovery checkpoints are separate.
use crate::engine::{Cached, Result};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

const MANAGED: [&str; 6] = [
    "artifacts",
    "bundles",
    "downloads",
    "manifests",
    "ime",
    "performance",
];

thread_local! {
    static CLEANUP_WARNING: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Backend maintenance diagnostics belong to the worker's opt-in details.
pub fn take_cleanup_warning() -> Option<String> {
    CLEANUP_WARNING.with(|warning| warning.borrow_mut().take())
}

pub(crate) fn prepared_cleanup(result: Result<()>) {
    if let Err(error) = result {
        CLEANUP_WARNING.with(|warning| {
            *warning.borrow_mut() = Some(format!("Download cache cleanup pending: {error}"))
        });
    }
}

pub(crate) fn cleanup_warning(message: String) {
    CLEANUP_WARNING.with(|warning| *warning.borrow_mut() = Some(message));
}

/// Callers must prove ownership before requesting recoverable transaction cleanup.
pub(crate) fn recycle_owned(root: &Path, paths: &[PathBuf]) -> Result<()> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    for path in paths {
        validate_tree(&root, path)?;
    }
    recycle(&root, paths)
}

pub(crate) struct Lease {
    _file: File,
}

pub(crate) struct Attempt {
    root: PathBuf,
    groups: Vec<String>,
    existing: HashSet<PathBuf>,
    complete: bool,
}
impl Attempt {
    pub(crate) fn begin(root: &Path, groups: &[&str]) -> Result<Self> {
        let mut existing = HashSet::new();
        for group in groups {
            if !MANAGED.contains(group) {
                return Err("Unknown cache owner".into());
            }
            let directory = root.join(group);
            if directory.exists() {
                validate_tree(&root.canonicalize().map_err(|e| e.to_string())?, &directory)?;
                let mut pending = vec![directory];
                while let Some(directory) = pending.pop() {
                    for item in fs::read_dir(directory).map_err(|e| e.to_string())? {
                        let path = item.map_err(|e| e.to_string())?.path();
                        if path.is_dir() {
                            pending.push(path.clone());
                        }
                        existing.insert(path);
                    }
                }
            }
        }
        Ok(Self {
            root: root.to_owned(),
            groups: groups.iter().map(|s| (*s).to_owned()).collect(),
            existing,
            complete: false,
        })
    }
    pub(crate) fn commit(&mut self) {
        self.complete = true;
    }
}
impl Drop for Attempt {
    fn drop(&mut self) {
        if self.complete {
            return;
        }
        let result = (|| -> Result<()> {
            let canonical = self.root.canonicalize().map_err(|e| e.to_string())?;
            let mut removals = Vec::new();
            let mut pending = self
                .groups
                .iter()
                .map(|name| self.root.join(name))
                .filter(|p| p.exists())
                .collect::<Vec<_>>();
            while let Some(directory) = pending.pop() {
                validate_tree(&canonical, &directory)?;
                for item in fs::read_dir(directory).map_err(|e| e.to_string())? {
                    let path = item.map_err(|e| e.to_string())?.path();
                    if !self.existing.contains(&path) {
                        removals.push(path);
                    } else if path.is_dir() {
                        pending.push(path);
                    }
                }
            }
            recycle(&self.root, &removals)
        })();
        if let Err(error) = result {
            eprintln!("Could not recycle incomplete download data: {error}");
            prepared_cleanup(Err(error));
        }
    }
}

fn plain(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("Cache links are not supported".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("Cache reparse points are not supported".into());
        }
    }
    Ok(())
}

// Also used for the updater's fixed, hash-checked sibling files. No recursion,
// shell UI or permanent-delete fallback; callers retain ownership checks.
pub(crate) fn recycle_siblings(root: &Path, paths: &[PathBuf]) -> Result<()> {
    for ancestor in root.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        plain(ancestor)?;
    }
    for path in paths {
        if path.parent() != Some(root) || !path.is_file() {
            return Err("Cleanup accepts only regular sibling files".into());
        }
        plain(path)?;
    }
    recycle(root, paths)
}

pub(crate) fn lock(root: &Path) -> Result<Lease> {
    for ancestor in root.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        if ancestor.exists() {
            plain(ancestor)?;
        }
    }
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let path = root.join("cache.lock");
    if path.exists() {
        plain(&path)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.try_lock()
        .map_err(|_| "Another operation is using downloaded files".to_string())?;
    Ok(Lease { _file: file })
}

pub(crate) fn register_game(cache: &Path, game: &Path) -> Result<()> {
    let path = cache.join("installed-games.json");
    let mut games: Vec<PathBuf> = if path.exists() {
        plain(&path)?;
        serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?
    } else {
        Vec::new()
    };
    if !games.iter().any(|p| p == game) {
        games.push(game.to_owned());
        crate::engine::save_settings(&path, &games)?;
    }
    Ok(())
}

pub(crate) fn no_pending_transactions(cache: &Path) -> Result<()> {
    let registry = cache.join("installed-games.json");
    let mut games: Vec<PathBuf> = if registry.exists() {
        plain(&registry)?;
        serde_json::from_slice(&fs::read(registry).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?
    } else {
        Vec::new()
    };
    let settings = cache.join("settings.json");
    if settings.exists() {
        plain(&settings)?;
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(settings).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if let Some(game) = value["game_folder"].as_str().filter(|s| !s.is_empty()) {
            games.push(PathBuf::from(game));
        }
    }
    if games.iter().any(|game| {
        game.join(".watchcat-localization/patcher-journal.json")
            .exists()
    }) {
        return Err("Recover interrupted localization before clearing downloads".into());
    }
    Ok(())
}

fn validate_tree(root: &Path, candidate: &Path) -> Result<()> {
    let mut pending = vec![candidate.to_owned()];
    while let Some(path) = pending.pop() {
        plain(&path)?;
        let actual = path.canonicalize().map_err(|e| e.to_string())?;
        if actual == root || !actual.starts_with(root) {
            return Err("Cache cleanup path escapes its owner".into());
        }
        if fs::symlink_metadata(&path)
            .map_err(|e| e.to_string())?
            .is_dir()
        {
            for entry in fs::read_dir(&path).map_err(|e| e.to_string())? {
                pending.push(entry.map_err(|e| e.to_string())?.path());
            }
        }
    }
    Ok(())
}

fn prune(root: &Path, groups: &[(&str, HashSet<PathBuf>)]) -> Result<()> {
    let canonical = root.canonicalize().map_err(|e| e.to_string())?;
    let mut removals = Vec::new();
    // Validate every candidate before any recycle operation, including all descendants.
    for (name, keep) in groups {
        if !MANAGED.contains(name) {
            return Err("Unknown cache owner".into());
        }
        let group = root.join(name);
        if !group.exists() {
            continue;
        }
        validate_tree(&canonical, &group)?;
        if !group.is_dir() {
            return Err("Managed cache owner is not a directory".into());
        }
        for item in fs::read_dir(&group).map_err(|e| e.to_string())? {
            let path = item.map_err(|e| e.to_string())?.path();
            let actual = path.canonicalize().map_err(|e| e.to_string())?;
            if !keep.contains(&actual) {
                removals.push(path);
            }
        }
    }
    recycle(root, &removals)
}

pub(crate) fn retain_translations(
    root: &Path,
    packages: &[&Cached],
    manifest: Option<&str>,
) -> Result<()> {
    no_pending_transactions(root)?;
    let mut artifacts = HashSet::new();
    let mut bundles = HashSet::new();
    let mut manifests = HashSet::new();
    for cached in packages {
        let directory = cached.directory.canonicalize().map_err(|e| e.to_string())?;
        let owner = root
            .join("bundles")
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if directory.parent() != Some(owner.as_path()) {
            return Err("Prepared bundle is outside its cache owner".into());
        }
        bundles.insert(directory);
        for file in &cached.package.files {
            artifacts.insert(
                root.join("artifacts")
                    .join(&file.artifact_sha256)
                    .canonicalize()
                    .map_err(|e| e.to_string())?,
            );
        }
    }
    if let Some(hash) = manifest {
        manifests.insert(
            root.join("manifests")
                .join(hash)
                .canonicalize()
                .map_err(|e| e.to_string())?,
        );
    }
    prune(
        root,
        &[
            ("artifacts", artifacts),
            ("bundles", bundles),
            ("manifests", manifests),
            ("downloads", HashSet::new()),
        ],
    )
}

pub(crate) fn retain_family(root: &Path, group: &str, current: &Path) -> Result<()> {
    let current = current.canonicalize().map_err(|e| e.to_string())?;
    let owner = root.join(group).canonicalize().map_err(|e| e.to_string())?;
    if current.parent() != Some(owner.as_path()) {
        return Err("Prepared package is outside its cache owner".into());
    }
    prune(root, &[(group, HashSet::from([current]))])
}

pub(crate) fn retain_ime_files(root: &Path, current: &Path) -> Result<()> {
    validate_tree(&root.canonicalize().map_err(|e| e.to_string())?, current)?;
    let mut removals = Vec::new();
    for item in fs::read_dir(current).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        if !["ime-package.json", crate::ime::DLL]
            .iter()
            .any(|name| path.file_name().is_some_and(|n| n == *name))
        {
            removals.push(path);
        }
    }
    recycle(root, &removals)
}

/// Recycle downloaded/prepared packages, leaving settings, logs and games intact.
pub fn clear_downloads(root: &Path) -> Result<()> {
    let _lease = lock(root)?;
    no_pending_transactions(root)?;
    prune(
        root,
        &MANAGED
            .iter()
            .map(|name| (*name, HashSet::new()))
            .collect::<Vec<_>>(),
    )
}

#[cfg(all(windows, not(test)))]
fn recycle(_root: &Path, paths: &[PathBuf]) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            UI::Shell::{
                FOF_NO_UI, FOF_WANTNUKEWARNING, FOFX_EARLYFAILURE, FOFX_RECYCLEONDELETE,
                FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName,
            },
        },
        core::PCWSTR,
    };
    if paths.is_empty() {
        return Ok(());
    }
    // Rust send-to-trash: never show Shell UI or silently fall back to hard deletion.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let _apartment = Apartment;
        let operation: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| e.to_string())?;
        operation
            .SetOperationFlags(
                FOF_NO_UI | FOF_WANTNUKEWARNING | FOFX_RECYCLEONDELETE | FOFX_EARLYFAILURE,
            )
            .map_err(|e| e.to_string())?;
        for path in paths {
            let text = path.to_string_lossy();
            let ordinary = if let Some(p) = text.strip_prefix(r"\\?\UNC\") {
                format!(r"\\{p}")
            } else if let Some(p) = text.strip_prefix(r"\\?\") {
                p.to_owned()
            } else {
                text.into_owned()
            };
            let wide = std::ffi::OsStr::new(&ordinary)
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>();
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)
                .map_err(|e| e.to_string())?;
            operation
                .DeleteItem(&item, None)
                .map_err(|e| e.to_string())?;
        }
        operation.PerformOperations().map_err(|e| e.to_string())?;
        if operation
            .GetAnyOperationsAborted()
            .map_err(|e| e.to_string())?
            .as_bool()
        {
            return Err("Some downloads could not be recycled".into());
        }
    }
    Ok(())
}

#[cfg(all(not(windows), not(test)))]
fn recycle(_root: &Path, paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        Ok(())
    } else {
        Err("Download cleanup requires the Windows Recycle Bin".into())
    }
}

#[cfg(test)]
fn recycle(root: &Path, paths: &[PathBuf]) -> Result<()> {
    // CPU fixtures exercise plans without deleting data or touching the real Recycle Bin.
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if !paths.is_empty() && root.join("recycle-failure-fixture").exists() {
        return Err("Synthetic recycle failure".into());
    }
    let destination = root
        .parent()
        .ok_or("Fixture parent missing")?
        .join("recycled-fixture-data");
    for (index, path) in paths.iter().enumerate() {
        fs::create_dir_all(&destination).map_err(|e| e.to_string())?;
        fs::rename(
            path,
            destination.join(NEXT.fetch_add(1, Ordering::Relaxed).to_string()),
        )
        .map_err(|e| e.to_string())?;
        if index == 0 && root.join("recycle-partial-failure-fixture").exists() {
            return Err("Synthetic partial recycle failure".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn fixture() -> PathBuf {
        let root = std::env::temp_dir()
            .join(format!(
                "paladinscat-cache-test-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ))
            .join("cache");
        fs::create_dir_all(&root).unwrap();
        root
    }
    #[test]
    fn clear_is_scoped_and_preserves_settings_and_game_data() {
        let root = fixture();
        for group in MANAGED {
            fs::create_dir(root.join(group)).unwrap();
            fs::write(root.join(group).join("old"), b"old").unwrap();
        }
        fs::write(root.join("settings.json"), b"{}").unwrap();
        fs::create_dir(root.join("launch-logs")).unwrap();
        fs::write(root.join("launch-logs/log"), b"log").unwrap();
        let game = root.parent().unwrap().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("original"), b"game").unwrap();
        clear_downloads(&root).unwrap();
        for group in MANAGED {
            assert_eq!(fs::read_dir(root.join(group)).unwrap().count(), 0);
        }
        assert_eq!(fs::read(root.join("settings.json")).unwrap(), b"{}");
        assert_eq!(fs::read(root.join("launch-logs/log")).unwrap(), b"log");
        assert_eq!(fs::read(game.join("original")).unwrap(), b"game");
    }
    #[test]
    fn clear_refuses_concurrent_work_and_pending_recovery_before_changes() {
        let root = fixture();
        let lease = lock(&root).unwrap();
        assert!(clear_downloads(&root).is_err());
        drop(lease);
        let game = root.parent().unwrap().join("game");
        fs::create_dir_all(game.join(".watchcat-localization")).unwrap();
        fs::write(
            game.join(".watchcat-localization/patcher-journal.json"),
            b"{}",
        )
        .unwrap();
        register_game(&root, &game).unwrap();
        fs::create_dir(root.join("artifacts")).unwrap();
        fs::write(root.join("artifacts/old"), b"required").unwrap();
        assert!(clear_downloads(&root).is_err());
        assert_eq!(fs::read(root.join("artifacts/old")).unwrap(), b"required");
    }
    #[test]
    fn retains_only_one_family_without_touching_other_families() {
        let root = fixture();
        fs::create_dir_all(root.join("ime/new")).unwrap();
        fs::create_dir(root.join("ime/old")).unwrap();
        fs::create_dir_all(root.join("performance/current")).unwrap();
        retain_family(&root, "ime", &root.join("ime/new")).unwrap();
        assert!(root.join("ime/new").exists());
        assert!(!root.join("ime/old").exists());
        assert!(root.join("performance/current").exists());
    }
    #[cfg(windows)]
    #[test]
    fn clear_refuses_nested_links_without_touching_the_foreign_target() {
        let root = fixture();
        let foreign = root.parent().unwrap().join("foreign");
        fs::write(&foreign, b"foreign").unwrap();
        fs::create_dir(root.join("artifacts")).unwrap();
        std::os::windows::fs::symlink_file(&foreign, root.join("artifacts/link")).unwrap();
        assert!(clear_downloads(&root).is_err());
        assert_eq!(fs::read(foreign).unwrap(), b"foreign");
    }
}
