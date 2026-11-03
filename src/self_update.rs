//! Stable GitHub releases, verified direct EXEs and an owned Windows transaction.
//! No credentials, shell script, elevation, game launch or version override.
use crate::{cache, engine::Result, sources};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(windows)]
mod windows;

pub const EXE: &str = "paladinscat-launcher.exe";
const STAGE: &str = "paladinscat-launcher.update.exe";
const HELPER: &str = "paladinscat-launcher.updater.exe";
const BACKUP: &str = "paladinscat-launcher.previous.exe";
const JOURNAL: &str = "paladinscat-launcher.update.json";
const JOURNAL_TEMP: &str = "paladinscat-launcher.update-journal.tmp";
const ARMED: &str = "paladinscat-launcher.update-armed.json";
const READY: &str = "paladinscat-launcher.update-ready.json";
const MAX_EXE: u64 = 128 * 1024 * 1024;
const MAX_METADATA: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub version: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", content = "release", rename_all = "snake_case")]
pub enum Check {
    NotPublished,
    Current,
    Available(Release),
}

// Release versions deliberately exclude prerelease/build labels. They must map
// exactly to the three Windows PE version components; no lexical comparisons.
fn version(value: &str) -> Result<[u16; 3]> {
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() != 3
        || parts.iter().any(|p| {
            p.is_empty()
                || !p.bytes().all(|c| c.is_ascii_digit())
                || (p.len() > 1 && p.starts_with('0'))
        })
    {
        return Err("Expected a stable major.minor.patch version".into());
    }
    Ok([
        parts[0]
            .parse()
            .map_err(|_| "Version component too large")?,
        parts[1]
            .parse()
            .map_err(|_| "Version component too large")?,
        parts[2]
            .parse()
            .map_err(|_| "Version component too large")?,
    ])
}

impl Release {
    fn validate(&self) -> Result<()> {
        version(&self.version)?;
        if self.sha256.len() != 64
            || !self.sha256.bytes().all(|c| c.is_ascii_hexdigit())
            || !(128..=MAX_EXE).contains(&self.bytes)
        {
            return Err("Launcher release checksum or size is invalid".into());
        }
        Ok(())
    }

    pub fn page(&self) -> String {
        format!(
            "https://github.com/{}/releases/tag/v{}",
            sources::LAUNCHER_REPOSITORY,
            self.version
        )
    }

    fn url(&self) -> String {
        format!(
            "https://github.com/{}/releases/download/v{}/{EXE}",
            sources::LAUNCHER_REPOSITORY,
            self.version
        )
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20))
        .https_only(true)
        .build()
}

pub fn check() -> Result<Check> {
    let response = match agent()
        .get(sources::LAUNCHER_RELEASE_API)
        .set(
            "User-Agent",
            concat!("PaladinsCat-Launcher/", env!("CARGO_PKG_VERSION")),
        )
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) => return Ok(Check::NotPublished),
        Err(error) => return Err(format!("Launcher update check: {error}")),
    };
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_METADATA + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    parse_release(&bytes, env!("CARGO_PKG_VERSION"))
}

pub fn parse_release(bytes: &[u8], installed: &str) -> Result<Check> {
    #[derive(Deserialize)]
    struct GithubRelease {
        tag_name: String,
        draft: bool,
        prerelease: bool,
        html_url: String,
        assets: Vec<Asset>,
    }
    #[derive(Deserialize)]
    struct Asset {
        name: String,
        browser_download_url: String,
        size: u64,
        digest: Option<String>,
        state: String,
    }
    if bytes.len() as u64 > MAX_METADATA {
        return Err("Launcher release metadata exceeds its limit".into());
    }
    let github: GithubRelease = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if github.draft || github.prerelease {
        return Err("Only published stable launcher releases are supported".into());
    }
    let latest = github
        .tag_name
        .strip_prefix('v')
        .ok_or("Launcher release tag must start with v")?;
    let latest_version = version(latest)?;
    let mut release = Release {
        version: latest.into(),
        sha256: String::new(),
        bytes: 0,
    };
    if github.html_url != release.page() {
        return Err("Launcher release belongs to an unexpected repository".into());
    }
    if latest_version <= version(installed)? {
        return Ok(Check::Current);
    }
    let assets = github
        .assets
        .iter()
        .filter(|asset| asset.name == EXE)
        .collect::<Vec<_>>();
    let [asset] = assets.as_slice() else {
        return Err("Release must contain exactly one direct launcher EXE".into());
    };
    if asset.browser_download_url != release.url() || asset.state != "uploaded" {
        return Err("Launcher EXE has an unexpected source or upload state".into());
    }
    release.sha256 = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .ok_or("GitHub did not supply the launcher EXE SHA-256")?
        .to_ascii_lowercase();
    release.bytes = asset.size;
    release.validate()?;
    Ok(Check::Available(release))
}

pub fn validate_executable(bytes: &[u8], expected_version: &str) -> Result<()> {
    let at = |offset: usize| {
        bytes
            .get(offset..offset.saturating_add(4))
            .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
    };
    let pe = at(0x3c).ok_or("PE header absent")? as usize;
    if bytes.get(..2) != Some(b"MZ")
        || bytes.get(pe..pe.saturating_add(4)) != Some(b"PE\0\0")
        || bytes.get(pe.saturating_add(4)..pe.saturating_add(6)) != Some(&[0x64, 0x86])
        || bytes.get(pe.saturating_add(24)..pe.saturating_add(26)) != Some(&[0x0b, 0x02])
    {
        return Err("Launcher must be a Windows x64 PE32+ executable".into());
    }
    let [major, minor, patch] = version(expected_version)?;
    if !bytes.windows(4).enumerate().any(|(offset, marker)| {
        marker == [0xbd, 0x04, 0xef, 0xfe]
            && at(offset + 8) == Some((u32::from(major) << 16) | u32::from(minor))
            && at(offset + 12) == Some(u32::from(patch) << 16)
    }) {
        return Err("Launcher PE version does not match its release tag".into());
    }
    Ok(())
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn plain(path: &Path) -> Result<()> {
    for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        let metadata = fs::symlink_metadata(ancestor).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("Launcher updates do not follow file links".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("Launcher updates do not follow reparse points".into());
            }
        }
    }
    Ok(())
}

fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    plain(path)?;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Updater file exceeds its size limit".into());
    }
    Ok(bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    plain(path.parent().ok_or("Updater directory absent")?)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("Cannot write update beside launcher: {e}"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema: u32,
    parent: u32,
    parent_created: u64,
    helper: u32,
    helper_created: u64,
    target: PathBuf,
    old_sha256: String,
    release: Release,
    phase: Phase,
    error: Option<String>,
    staged_sha256: Option<String>,
    #[serde(default)]
    owned_files: std::collections::BTreeMap<String, [u32; 3]>,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Phase {
    Prepared,
    Replacing,
    Starting,
    Committed,
    RolledBack,
}

impl Journal {
    fn root(&self) -> &Path {
        self.target.parent().expect("validated updater root")
    }
    fn load(root: &Path) -> Result<Self> {
        let journal: Self = serde_json::from_slice(&read(&root.join(JOURNAL), 16384)?)
            .map_err(|e| e.to_string())?;
        journal.release.validate()?;
        if journal.schema != 1
            || journal.target != root.join(EXE)
            || journal.old_sha256.len() != 64
            || !journal.old_sha256.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err("Invalid launcher update journal".into());
        }
        Ok(journal)
    }
    fn save(&self) -> Result<()> {
        let root = self.root();
        let temp = root.join(JOURNAL_TEMP);
        if temp.exists() {
            let prior: Self = serde_json::from_slice(&read(&temp, 16384)?)
                .map_err(|_| "Incomplete update journal staging was preserved for recovery")?;
            if prior.target != self.target
                || prior.old_sha256 != self.old_sha256
                || prior.release.sha256 != self.release.sha256
                || prior.parent_created != self.parent_created
            {
                return Err("Update journal staging has a different owner".into());
            }
            cache::recycle_siblings(root, std::slice::from_ref(&temp))?;
        }
        write_new(
            &temp,
            &serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )?;
        #[cfg(windows)]
        if root.join(JOURNAL).exists() {
            return windows::replace(&root.join(JOURNAL), &temp, None);
        }
        fs::rename(temp, root.join(JOURNAL)).map_err(|e| e.to_string())
    }
}

fn matches(path: &Path, hash: &str) -> Result<()> {
    if sha(&read(path, MAX_EXE)?) != hash {
        return Err(format!("Updater file hash changed: {}", path.display()));
    }
    Ok(())
}

fn verify_payload(path: &Path, release: &Release) -> Result<()> {
    let bytes = read(path, MAX_EXE)?;
    if bytes.len() as u64 != release.bytes || sha(&bytes) != release.sha256 {
        return Err("Launcher download checksum or size mismatch".into());
    }
    validate_executable(&bytes, &release.version)
}

#[cfg(test)]
fn receive_payload(
    mut input: impl Read,
    target: &Path,
    release: &Release,
    progress: &dyn Fn(u8),
) -> Result<()> {
    release.validate()?;
    plain(target.parent().ok_or("Updater directory absent")?)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|e| format!("Launcher folder is not writable: {e}"))?;
    receive_into(&mut input, &mut output, release, progress)?;
    drop(output);
    verify_payload(target, release)
}

fn receive_into(
    mut input: impl Read,
    output: &mut File,
    release: &Release,
    progress: &dyn Fn(u8),
) -> Result<()> {
    let mut total = 0;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 131072];
    loop {
        // The extra byte detects excess data without permitting an unbounded stream.
        let permitted = (release.bytes + 1 - total).min(buffer.len() as u64) as usize;
        let count = input
            .read(&mut buffer[..permitted])
            .map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > release.bytes {
            return Err("Launcher download exceeds its declared size".into());
        }
        output
            .write_all(&buffer[..count])
            .map_err(|e| e.to_string())?;
        digest.update(&buffer[..count]);
        progress((100 * total / release.bytes) as u8);
    }
    output.sync_all().map_err(|e| e.to_string())?;
    if total != release.bytes || format!("{:x}", digest.finalize()) != release.sha256 {
        return Err("Launcher download checksum or size mismatch".into());
    }
    Ok(())
}

/// Downloads/stages beside the EXE, then returns only after the helper has opened
/// the exact parent process and acknowledged that it is ready to wait for exit.
/// The GUI must close normally only after this succeeds.
#[cfg(windows)]
pub fn install(release: Release, progress: &dyn Fn(u8)) -> Result<()> {
    let target = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if target.file_name().and_then(|n| n.to_str()) != Some(EXE) {
        return Err("Keep the launcher filename paladinscat-launcher.exe to update it".into());
    }
    if std::env::args_os().len() != 1 {
        return Err(
            "Developer proof/preview launches require their original launch procedure".into(),
        );
    }
    cleanup(&target)?;
    let root = target.parent().ok_or("Launcher folder absent")?;
    plain(&target)?;
    for name in [STAGE, HELPER, BACKUP, JOURNAL, JOURNAL_TEMP, ARMED, READY] {
        if root.join(name).exists() {
            return Err(format!("An earlier update needs recovery: {name}"));
        }
    }
    release.validate()?;
    if version(&release.version)? <= version(env!("CARGO_PKG_VERSION"))? {
        return Err("Launcher update must be newer than the running version".into());
    }
    let original = read(&target, MAX_EXE)?;
    validate_executable(&original, env!("CARGO_PKG_VERSION"))?;
    let mut journal = Journal {
        schema: 1,
        parent: std::process::id(),
        parent_created: windows::Process::open(std::process::id())?.created()?,
        helper: 0,
        helper_created: 0,
        target: target.clone(),
        old_sha256: sha(&original),
        release,
        phase: Phase::Prepared,
        error: None,
        staged_sha256: None,
        owned_files: Default::default(),
    };
    // create_new reserves the transaction before any download or cleanup.
    write_new(
        &root.join(JOURNAL),
        &serde_json::to_vec_pretty(&journal).map_err(|e| e.to_string())?,
    )?;
    let staged = (|| -> Result<()> {
        // Open the stage ourselves before download. Even a partial body has
        // recorded ownership; a collision never becomes cleanup permission.
        let mut stage = windows::create_owned(&mut journal, STAGE)?;
        let downloaded = (|| {
            let response = agent()
                .get(&journal.release.url())
                .call()
                .map_err(|e| e.to_string())?;
            if response
                .header("Content-Length")
                .and_then(|v| v.parse::<u64>().ok())
                .is_some_and(|n| n != journal.release.bytes)
            {
                return Err("Launcher download Content-Length mismatch".into());
            }
            receive_into(
                response.into_reader(),
                &mut stage,
                &journal.release,
                progress,
            )
        })();
        drop(stage);
        journal.staged_sha256 = Some(sha(&read(&root.join(STAGE), MAX_EXE)?));
        journal.save()?;
        downloaded?;
        verify_payload(&root.join(STAGE), &journal.release)?;
        let mut helper = windows::create_owned(&mut journal, HELPER)?;
        helper
            .write_all(&original)
            .and_then(|_| helper.sync_all())
            .map_err(|e| e.to_string())?;
        drop(helper);
        windows::arm(&mut journal)
    })();
    if let Err(error) = staged {
        journal.error = Some(error.clone());
        // Preserve a failed/incomplete download for explicit, ownership-aware
        // cleanup; never close the current GUI after a staging error.
        let _ = journal.save();
        let _ = cleanup(&target);
        return Err(error);
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn install(_: Release, _: &dyn Fn(u8)) -> Result<()> {
    Err("Launcher updates require Windows".into())
}

/// Runs before any renderer allocation. Internal helper failures never enter GUI.
#[cfg(windows)]
pub fn helper_mode(args: &[String]) -> Option<Result<()>> {
    if args.get(1).map(String::as_str) != Some("--apply-launcher-update") {
        return None;
    }
    Some((|| {
        if args.len() != 2 {
            return Err("Unexpected updater arguments".into());
        }
        windows::apply()
    })())
}
#[cfg(not(windows))]
pub fn helper_mode(_: &[String]) -> Option<Result<()>> {
    None
}

/// Called after the first normal UI frame, and after renderer identity checks.
/// A PID/hash-bound marker is consumed by the waiting helper, never by a URL.
#[cfg(windows)]
pub fn mark_ready() -> Result<()> {
    windows::ready()
}
#[cfg(not(windows))]
pub fn mark_ready() -> Result<()> {
    Ok(())
}

fn cleanup(target: &Path) -> Result<()> {
    let root = target.parent().ok_or("Launcher directory absent")?;
    if !root.join(JOURNAL).exists() {
        return Ok(());
    }
    let journal = Journal::load(root)?;
    #[cfg(windows)]
    if journal.phase == Phase::Prepared
        && journal.error.is_none()
        && windows::Process::same_live(journal.parent, journal.parent_created)?
    {
        return Err("Launcher update download is still owned by a running launcher".into());
    }
    #[cfg(windows)]
    if journal.helper != 0 && windows::Process::same_live(journal.helper, journal.helper_created)? {
        return Err("Launcher updater is still running".into());
    }
    let current = sha(&read(target, MAX_EXE)?);
    let hash = match journal.phase {
        Phase::Prepared | Phase::RolledBack => &journal.old_sha256,
        Phase::Committed => &journal.release.sha256,
        // A dead helper or interrupted machine can leave a valid new image.
        // A normal successful startup commits it; failures retain the backup.
        Phase::Replacing | Phase::Starting => {
            return Err("Interrupted update: keep the previous EXE for recovery".into());
        }
    };
    if &current != hash {
        return Err(
            "Launcher differs from its update journal; recovery files were preserved".into(),
        );
    }
    let mut paths = Vec::new();
    let temp = root.join(JOURNAL_TEMP);
    if temp.exists() {
        let pending: Journal = serde_json::from_slice(&read(&temp, 16384)?)
            .map_err(|_| "Incomplete update journal staging was preserved for recovery")?;
        if pending.target != journal.target
            || pending.old_sha256 != journal.old_sha256
            || pending.release.sha256 != journal.release.sha256
            || pending.parent_created != journal.parent_created
        {
            return Err("Update journal staging has a different owner".into());
        }
        paths.push(temp);
    }
    for (name, expected) in [
        (HELPER, Some(journal.old_sha256.as_str())),
        (BACKUP, Some(journal.old_sha256.as_str())),
        (STAGE, journal.staged_sha256.as_deref()),
        (ARMED, None),
        (READY, None),
    ] {
        let path = root.join(name);
        if !path.exists() {
            continue;
        }
        plain(&path)?;
        let complete = expected.is_some_and(|hash| matches(&path, hash).is_ok());
        #[cfg(windows)]
        let owned_partial = journal.phase == Phase::Prepared
            && journal
                .owned_files
                .get(name)
                .is_some_and(|id| windows::file_identity(&path).is_ok_and(|actual| &actual == id))
            && fs::metadata(&path).map_err(|e| e.to_string())?.len() <= MAX_EXE;
        #[cfg(not(windows))]
        let owned_partial = false;
        if matches!(name, STAGE | HELPER | BACKUP) && !complete && !owned_partial {
            return Err("Interrupted updater file ownership could not be verified; recovery files were preserved".into());
        }
        paths.push(path);
    }
    cache::recycle_siblings(root, &paths)?;
    // Journal last: partial recycle failures retain ownership and remain retryable.
    cache::recycle_siblings(root, &[root.join(JOURNAL)])
}

/// Cleanup runs on a worker after readiness; the helper must exit first. Fixed
/// sibling names bound retained data to one transaction even on repeated errors.
pub fn startup_cleanup() -> Result<Option<String>> {
    let target = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        let root = target.parent().ok_or("Launcher directory absent")?;
        if root.join(JOURNAL).exists() {
            let journal = Journal::load(root)?;
            if journal.helper != 0
                && windows::Process::same_live(journal.helper, journal.helper_created)?
            {
                windows::Process::open(journal.helper)?.wait(Duration::from_secs(45))?;
            }
        }
    }
    let root = target.parent().ok_or("Launcher directory absent")?;
    let notice = if root.join(JOURNAL).exists() {
        Journal::load(root)?.error
    } else {
        None
    };
    cleanup(&target)?;
    Ok(notice)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    pub(super) fn directory() -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/self-update-tests")
            .join(format!(
                "{}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }
    pub(super) fn image(v: [u16; 3]) -> Vec<u8> {
        let mut bytes = vec![0; 128];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&[0x64, 0x86]);
        bytes[88..90].copy_from_slice(&[0x0b, 0x02]);
        bytes[96..100].copy_from_slice(&[0xbd, 0x04, 0xef, 0xfe]);
        bytes[104..108].copy_from_slice(&((u32::from(v[0]) << 16) | u32::from(v[1])).to_le_bytes());
        bytes[108..112].copy_from_slice(&(u32::from(v[2]) << 16).to_le_bytes());
        bytes
    }
    pub(super) fn release() -> Release {
        Release {
            version: "0.3.9".into(),
            sha256: sha(&image([0, 3, 9])),
            bytes: 128,
        }
    }
    fn metadata() -> serde_json::Value {
        let r = release();
        serde_json::json!({"tag_name":"v0.3.9","draft":false,"prerelease":false,"html_url":r.page(),"assets":[{"name":EXE,"state":"uploaded","size":r.bytes,"digest":format!("sha256:{}",r.sha256),"browser_download_url":r.url()}]})
    }
    #[test]
    fn release_api_rejects_wrong_repo_missing_digest_duplicate_and_preview_assets() {
        let valid = metadata();
        let parse = |v: &serde_json::Value| parse_release(&serde_json::to_vec(v).unwrap(), "0.3.8");
        assert!(matches!(parse(&valid).unwrap(), Check::Available(_)));
        for field in ["draft", "prerelease"] {
            let mut changed = valid.clone();
            changed[field] = true.into();
            assert!(parse(&changed).is_err());
        }
        for value in [
            serde_json::Value::Null,
            "md5:1234".into(),
            format!("sha256:{}", "x".repeat(64)).into(),
        ] {
            let mut changed = valid.clone();
            changed["assets"][0]["digest"] = value;
            assert!(parse(&changed).is_err());
        }
        for url in [
            "https://example.com/launcher.exe",
            "http://github.com/NabiCook/PaladinsCat-Launcher/releases/download/v0.3.9/paladinscat-launcher.exe",
            "https://github.com/NabiCook/PaladinsCat-locales/releases/download/v0.3.9/paladinscat-launcher.exe",
        ] {
            let mut changed = valid.clone();
            changed["assets"][0]["browser_download_url"] = url.into();
            assert!(parse(&changed).is_err());
        }
        let mut changed = valid.clone();
        changed["assets"]
            .as_array_mut()
            .unwrap()
            .push(valid["assets"][0].clone());
        assert!(parse(&changed).is_err());
        let mut changed = valid.clone();
        changed["assets"][0]["name"] = "launcher.zip".into();
        assert!(parse(&changed).is_err());
        let mut changed = valid.clone();
        changed["assets"][0]["size"] = (MAX_EXE + 1).into();
        assert!(parse(&changed).is_err());
    }
    #[test]
    fn stable_versions_are_numeric_and_never_downgrade_or_replace_equal_builds() {
        let bytes = serde_json::to_vec(&metadata()).unwrap();
        for installed in ["0.3.9", "0.3.10", "1.0.0"] {
            assert!(matches!(
                parse_release(&bytes, installed).unwrap(),
                Check::Current
            ));
        }
        assert!(matches!(
            parse_release(&bytes, "0.3.8").unwrap(),
            Check::Available(_)
        ));
        assert!(version("0.3.10").unwrap() > version("0.3.9").unwrap());
        for invalid in [
            "0.3.9-beta",
            "0.3.9+build",
            "00.3.9",
            "0.3",
            "65536.0.0",
            "../0.3.9",
            "0.3.9/next",
        ] {
            assert!(version(invalid).is_err());
        }
    }
    #[test]
    fn corrupted_truncated_oversized_or_wrong_version_downloads_leave_current_exe_untouched() {
        let root = directory();
        let original = image([0, 3, 8]);
        fs::write(root.join(EXE), &original).unwrap();
        let r = release();
        let good = image([0, 3, 9]);
        let mut oversized = good.clone();
        oversized.push(0);
        let mut corrupted = good.clone();
        corrupted[1] = 0;
        for (index, body) in [&good[..127], oversized.as_slice(), corrupted.as_slice()]
            .into_iter()
            .enumerate()
        {
            let stage = root.join(format!("bad-{index}.exe"));
            assert!(receive_payload(body, &stage, &r, &|_| {}).is_err());
            assert_eq!(fs::read(root.join(EXE)).unwrap(), original);
        }
        receive_payload(good.as_slice(), &root.join(STAGE), &r, &|_| {}).unwrap();
        assert!(receive_payload(good.as_slice(), &root.join(STAGE), &r, &|_| {}).is_err());
        let wrong = image([0, 3, 8]);
        let wrong_release = Release {
            sha256: sha(&wrong),
            ..r
        };
        assert!(
            receive_payload(
                wrong.as_slice(),
                &root.join("wrong-version.exe"),
                &wrong_release,
                &|_| {}
            )
            .is_err()
        );
    }
    #[test]
    fn cleanup_checks_journal_target_and_every_backup_before_mutating_anything() {
        let root = directory();
        let old = image([0, 3, 8]);
        fs::write(root.join(EXE), &old).unwrap();
        fs::write(root.join(BACKUP), b"unexplained file").unwrap();
        let mut journal = Journal {
            schema: 1,
            parent: 0,
            parent_created: 0,
            helper: 0,
            helper_created: 0,
            target: root.join(EXE),
            old_sha256: sha(&old),
            release: release(),
            phase: Phase::RolledBack,
            error: Some("fixture".into()),
            staged_sha256: None,
            owned_files: Default::default(),
        };
        journal.save().unwrap();
        assert!(cleanup(&root.join(EXE)).is_err());
        assert_eq!(fs::read(root.join(BACKUP)).unwrap(), b"unexplained file");
        assert!(root.join(JOURNAL).exists());
        journal.target = root.join("another.exe");
        crate::engine::save_settings(&root.join(JOURNAL), &journal).unwrap();
        assert!(Journal::load(&root).is_err());
    }
    #[test]
    fn completed_update_recycles_only_owned_fixed_files_and_preserves_settings() {
        let root = directory();
        let old = image([0, 3, 8]);
        let new = image([0, 3, 9]);
        fs::write(root.join(EXE), &new).unwrap();
        fs::write(root.join(BACKUP), &old).unwrap();
        fs::write(root.join(HELPER), &old).unwrap();
        fs::write(root.join("settings.json"), b"preferences").unwrap();
        fs::write(root.join("unrelated.zip"), b"other download").unwrap();
        let journal = Journal {
            schema: 1,
            parent: 0,
            parent_created: 0,
            helper: 0,
            helper_created: 0,
            target: root.join(EXE),
            old_sha256: sha(&old),
            release: release(),
            phase: Phase::Committed,
            error: None,
            staged_sha256: None,
            owned_files: Default::default(),
        };
        journal.save().unwrap();
        cleanup(&root.join(EXE)).unwrap();
        for name in [BACKUP, HELPER, JOURNAL] {
            assert!(!root.join(name).exists());
        }
        assert_eq!(fs::read(root.join(EXE)).unwrap(), new);
        assert_eq!(
            fs::read(root.join("settings.json")).unwrap(),
            b"preferences"
        );
        assert!(root.join("unrelated.zip").exists());
    }
}
