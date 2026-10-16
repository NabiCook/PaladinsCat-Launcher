//! Tempest-compatible imports, adapted from ModV1Installer/ModV2Installer and
//! IniPatcher at LowRezStudio/Tempest 01081efe (GPL-3.0-or-later). No CLI dependency.
use crate::{
    cache,
    engine::{self, Result},
    game_discovery, launch, upk_patcher,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
const REGISTRY: &str = ".tempest/mods/mods.json";
const TXN: &str = ".tempest/paladinscat-mod-transaction";
const LIMIT: u64 = 128 * 1024 * 1024;
const TOTAL: u64 = 256 * 1024 * 1024;
type Files = BTreeMap<String, Vec<u8>>;
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub kind: String,
    pub enabled: bool,
}
impl ModInfo {
    pub fn managed(&self) -> bool {
        launch::family(&self.id) || launch::family(&self.name)
    }
}
fn text<'a>(record: &'a Value, key: &str) -> &'a str {
    record[key].as_str().unwrap_or("")
}
fn relative(name: &str) -> Result<String> {
    let name = name.replace('\\', "/");
    if name.len() > 240
        || name.split('/').any(|p| {
            p.is_empty()
                || p == "."
                || p == ".."
                || p.ends_with(['.', ' '])
                || p.chars()
                    .any(|c| c.is_control() || r#":*?"<>|"#.contains(c))
                || {
                    let base = p.split('.').next().unwrap().to_ascii_uppercase();
                    ["CON", "PRN", "AUX", "NUL"].contains(&base.as_str())
                        || (base.len() == 4
                            && (base.starts_with("COM") || base.starts_with("LPT"))
                            && base.as_bytes()[3].is_ascii_digit())
                }
        })
    {
        return Err("Unsafe mod path".into());
    }
    Ok(name)
}
fn safe(root: &Path, name: &str) -> Result<PathBuf> {
    let name = relative(name)?;
    let path = root.join(name);
    for ancestor in path.ancestors().take_while(|p| *p != root) {
        if let Ok(meta) = fs::symlink_metadata(ancestor) {
            #[cfg(windows)]
            let linked = {
                use std::os::windows::fs::MetadataExt;
                meta.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let linked = meta.file_type().is_symlink();
            if linked {
                return Err("Mod path contains a junction or symbolic link".into());
            }
        }
    }
    Ok(path)
}
fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = File::open(path).map_err(error)?;
    if file.metadata().map_err(error)?.len() > limit {
        return Err("Mod file exceeds size limit".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 > limit {
        return Err("Mod file exceeds size limit".into());
    }
    Ok(bytes)
}
fn optional(path: &Path) -> Result<Option<Vec<u8>>> {
    if path.exists() {
        read(path, LIMIT).map(Some)
    } else {
        Ok(None)
    }
}
fn records(root: &Path) -> Result<Vec<Value>> {
    let path = safe(root, REGISTRY)?;
    let records: Vec<Value> = match optional(&path)? {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(error)?,
        None => Vec::new(),
    };
    let mut ids = BTreeSet::new();
    if records.len() > 4096 {
        return Err("Too many registered mods".into());
    }
    for record in &records {
        let id = text(record, "Id");
        if !record.is_object() || (!id.is_empty() && !ids.insert(id.to_lowercase())) {
            return Err("Invalid or duplicate mod registry entry".into());
        }
    }
    Ok(records)
}
pub fn list(game: &Path) -> Result<Vec<ModInfo>> {
    let root = game.canonicalize().map_err(error)?;
    if !game_discovery::is_game_root(&root) {
        return Err("Select a valid game folder".into());
    }
    Ok(records(&root)?
        .iter()
        .map(|r| ModInfo {
            id: text(r, "Id").into(),
            name: text(r, "Name").into(),
            version: text(r, "Version").into(),
            author: text(r, "Author").into(),
            kind: text(r, "Kind").into(),
            enabled: r["Enabled"].as_bool().unwrap_or(true),
        })
        .collect())
}
#[derive(Serialize, Deserialize)]
struct Change {
    path: String,
    before: Option<String>,
    after: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    committed: bool,
    changes: Vec<Change>,
    created: Vec<String>,
}
pub fn recovery_pending(game: &Path) -> bool {
    let path = game.join(TXN).join("journal.json");
    if !path.exists() {
        return game.join(TXN).exists();
    }
    read(&path, 4 * 1024 * 1024)
        .ok()
        .and_then(|b| serde_json::from_slice::<Journal>(&b).ok())
        .is_none_or(|j| !j.committed)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(error)?;
    file.write_all(bytes).map_err(error)?;
    file.sync_all().map_err(error)
}
fn save_journal(root: &Path, j: &Journal) -> Result<()> {
    let path = safe(root, &format!("{TXN}/journal.json"))?;
    let next = safe(root, &format!("{TXN}/journal.next"))?;
    let bytes = serde_json::to_vec(j).map_err(error)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("Mod journal exceeds size limit".into());
    }
    write_new(&next, &bytes)?;
    fs::rename(next, path).map_err(error)
}
fn allowed_change(name: &str) -> bool {
    name == REGISTRY
        || name.starts_with(".tempest/v1/")
        || name.starts_with(".tempest/v2/")
        || name.starts_with("ChaosGame/")
        || name.starts_with("Engine/Config/")
}
fn recover_inner(root: &Path) -> Result<()> {
    let dir = safe(root, TXN)?;
    if !dir.exists() {
        return Ok(());
    }
    if !dir.join("journal.json").exists() {
        // Applying files requires a durable journal; only an empty preparation
        // layout can be safely retired without that journal.
        for entry in fs::read_dir(&dir).map_err(error)? {
            let path = entry.map_err(error)?.path();
            let name = path
                .file_name()
                .and_then(|p| p.to_str())
                .ok_or("Invalid mod staging name")?;
            if !((matches!(name, "old" | "new")
                && path.is_dir()
                && fs::read_dir(&path).map_err(error)?.next().is_none())
                || (name == "journal.next" && path.is_file()))
            {
                return Err(
                    "Mod recovery metadata is missing; preserve staging for inspection".into(),
                );
            }
        }
        return cache::recycle_owned(root, &[dir]);
    }
    let j: Journal = serde_json::from_slice(&read(
        &safe(root, &format!("{TXN}/journal.json"))?,
        4 * 1024 * 1024,
    )?)
    .map_err(error)?;
    if j.changes.len() > 8192 {
        return Err("Invalid mod recovery journal".into());
    }
    if !j.committed {
        for (i, c) in j.changes.iter().enumerate().rev() {
            if !allowed_change(&c.path) {
                return Err("Invalid mod recovery target".into());
            }
            let target = safe(root, &c.path)?;
            let current = optional(&target)?;
            let current_hash = current.as_deref().map(hash);
            if current_hash == c.before {
                continue;
            }
            if current_hash != c.after {
                return Err("A mod transaction file changed; preserve it before recovery".into());
            }
            if let Some(before) = &c.before {
                let bytes = read(&safe(root, &format!("{TXN}/old/{i}"))?, LIMIT)?;
                if hash(&bytes) != *before {
                    return Err("Mod recovery backup changed".into());
                }
                let temp = safe(root, &format!("{TXN}/restore-{i}"))?;
                write_new(&temp, &bytes)?;
                fs::rename(temp, target).map_err(error)?;
            } else {
                cache::recycle_owned(root, &[target])?;
            }
        }
        for name in j.created.iter().rev() {
            if !allowed_change(&format!("{name}/member")) {
                continue;
            }
            let path = safe(root, name)?;
            if path.is_dir() && fs::read_dir(&path).map_err(error)?.next().is_none() {
                cache::recycle_owned(root, &[path])?;
            }
        }
    }
    cache::recycle_owned(root, &[dir])
}
pub fn recover(game: &Path) -> Result<Vec<ModInfo>> {
    let (root, _lock) = engine::acquire_for_mod_recovery(game, &upk_patcher::is_game_running)?;
    recover_inner(&root)?;
    list(&root)
}
fn transact(root: &Path, files: Files, fail_after: Option<usize>) -> Result<()> {
    transact_changes(
        root,
        files.into_iter().map(|(p, b)| (p, Some(b))).collect(),
        fail_after,
    )
}
fn transact_changes(
    root: &Path,
    files: BTreeMap<String, Option<Vec<u8>>>,
    fail_after: Option<usize>,
) -> Result<()> {
    if files.len() > 8192 || files.values().flatten().any(|b| b.len() as u64 > LIMIT) {
        return Err("Mod transaction exceeds supported file count/size".into());
    }
    let mut entries = files.into_iter().collect::<Vec<_>>();
    entries.sort_by_key(|(p, _)| p == REGISTRY);
    let mut j = Journal {
        committed: false,
        changes: Vec::new(),
        created: Vec::new(),
    };
    let mut backups = Vec::new();
    let mut needed = 0;
    for (name, bytes) in &entries {
        if !allowed_change(name) {
            return Err("Mod target is outside supported game/mod directories".into());
        }
        let path = safe(root, name)?;
        let before = optional(&path)?;
        needed += bytes.as_ref().map_or(0, |b| b.len()) as u64
            + before.as_ref().map_or(0, |b| b.len()) as u64;
        if needed > TOTAL * 2 {
            return Err("Mod transaction exceeds size limit".into());
        }
        j.changes.push(Change {
            path: name.clone(),
            before: before.as_deref().map(hash),
            after: bytes.as_deref().map(hash),
        });
        backups.push(before);
        let mut missing = path.parent().unwrap();
        let mut dirs = Vec::new();
        while missing != root && !missing.exists() {
            dirs.push(
                missing
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
            missing = missing.parent().unwrap();
        }
        for name in dirs.into_iter().rev() {
            if !j.created.contains(&name) {
                j.created.push(name)
            }
        }
    }
    let dir = safe(root, TXN)?;
    fs::create_dir_all(dir.join("old")).map_err(error)?;
    fs::create_dir_all(dir.join("new")).map_err(error)?;
    if let Err(e) = save_journal(root, &j) {
        cache::recycle_owned(root, &[dir])?;
        return Err(e);
    }
    let staging = (|| {
        for (i, (_, bytes)) in entries.iter().enumerate() {
            if let Some(bytes) = bytes {
                write_new(&dir.join("new").join(i.to_string()), bytes)?;
            }
            if let Some(before) = &backups[i] {
                write_new(&dir.join("old").join(i.to_string()), before)?
            }
        }
        Ok::<_, String>(())
    })();
    if let Err(e) = staging {
        recover_inner(root)?;
        return Err(e);
    }
    let result = (|| {
        for (i, c) in j.changes.iter().enumerate() {
            if fail_after == Some(i) {
                return Err("Synthetic mod transaction interruption".into());
            }
            let target = safe(root, &c.path)?;
            if optional(&target)?.as_deref().map(hash) != c.before {
                return Err("Mod target changed during operation".into());
            }
            fs::create_dir_all(target.parent().unwrap()).map_err(error)?;
            if c.after.is_some() {
                fs::rename(dir.join("new").join(i.to_string()), target).map_err(error)?;
            } else if c.before.is_some() {
                // Keep removed bytes in the transaction until commit. Recovery
                // can restore them; committed staging goes to the Recycle Bin.
                fs::rename(target, dir.join("new").join(i.to_string())).map_err(error)?;
            }
        }
        j.committed = true;
        save_journal(root, &j)
    })();
    if let Err(e) = result {
        return match recover_inner(root) {
            Ok(()) => Err(format!("{e}; previous files restored")),
            Err(r) => Err(format!("{e}; mod recovery pending: {r}")),
        };
    }
    if let Err(e) = recover_inner(root) {
        cache::cleanup_warning(format!("Mod transaction cleanup pending: {e}"));
    }
    Ok(())
}

/// Preserve UTF-8/UTF-16 BOM and newline style; apply Tempest +,-,!,plain INI entries.
fn decode_ini(bytes: &[u8]) -> Result<(String, u8)> {
    let encoding = if bytes.starts_with(&[255, 254]) {
        1
    } else if bytes.starts_with(&[254, 255]) {
        2
    } else if bytes.starts_with(&[239, 187, 191]) {
        3
    } else {
        0
    };
    let text = if encoding == 1 || encoding == 2 {
        if !bytes.len().is_multiple_of(2) {
            return Err("Invalid UTF-16 INI".into());
        }
        let words = bytes[2..]
            .chunks_exact(2)
            .map(|b| {
                if encoding == 1 {
                    u16::from_le_bytes([b[0], b[1]])
                } else {
                    u16::from_be_bytes([b[0], b[1]])
                }
            })
            .collect::<Vec<_>>();
        String::from_utf16(&words).map_err(error)?
    } else {
        String::from_utf8(bytes[if encoding == 3 { 3 } else { 0 }..].to_vec()).map_err(error)?
    };
    Ok((text, encoding))
}
fn ini_entry(line: &str) -> Option<(&str, &str)> {
    let (key, val) = line.trim().split_once('=')?;
    let key = key.trim().trim_start_matches(['+', '-', '!']);
    (!key.is_empty() && !line.trim().starts_with([';', '#']))
        .then_some((key, val.trim().trim_end_matches(" ; tempest")))
}
fn encode_ini(text: String, encoding: u8) -> Vec<u8> {
    match encoding {
        1 => [
            vec![255, 254],
            text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        ]
        .concat(),
        2 => [
            vec![254, 255],
            text.encode_utf16().flat_map(u16::to_be_bytes).collect(),
        ]
        .concat(),
        3 => [vec![239, 187, 191], text.into_bytes()].concat(),
        _ => text.into_bytes(),
    }
}
fn patch_ini(original: &[u8], patch: &[u8]) -> Result<Vec<u8>> {
    let entry = ini_entry;
    let (text, encoding) = decode_ini(original)?;
    let (patch, _) = decode_ini(patch)?;
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines = text.lines().map(str::to_owned).collect::<Vec<_>>();
    let mut section = String::new();
    for line in patch.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_owned();
            continue;
        }
        let Some((key, value)) = entry(line) else {
            continue;
        };
        if section.is_empty() {
            return Err("Mod INI entry has no section".into());
        }
        let header = format!("[{section}]");
        let start = lines
            .iter()
            .position(|l| l.trim().eq_ignore_ascii_case(&header))
            .unwrap_or_else(|| {
                lines.push(header);
                lines.len() - 1
            });
        let end = (start + 1..lines.len())
            .find(|i| lines[*i].trim().starts_with('['))
            .unwrap_or(lines.len());
        let matching = (start + 1..end)
            .filter(|i| entry(&lines[*i]).is_some_and(|(k, _)| k.eq_ignore_ascii_case(key)))
            .collect::<Vec<_>>();
        match line.as_bytes()[0] {
            b'+' => {
                if !matching
                    .iter()
                    .any(|i| entry(&lines[*i]).unwrap().1.eq_ignore_ascii_case(value))
                {
                    lines.insert(end, format!("+{key}={value}"));
                }
            }
            b'-' | b'!' => {
                for i in matching.into_iter().rev() {
                    if line.starts_with('!')
                        || entry(&lines[i]).unwrap().1.eq_ignore_ascii_case(value)
                    {
                        lines.remove(i);
                    }
                }
            }
            _ => {
                if let Some(i) = matching.first() {
                    lines[*i] = format!("{key}={value}")
                } else {
                    lines.insert(end, format!("{key}={value}"))
                }
            }
        }
    }
    let text = lines.join(newline) + newline;
    Ok(encode_ini(text, encoding))
}

fn archive(path: &Path) -> Result<Files> {
    let file = File::open(path).map_err(error)?;
    if file.metadata().map_err(error)?.len() > LIMIT {
        return Err("Mod archive exceeds size limit".into());
    }
    let mut zip = zip::ZipArchive::new(file).map_err(error)?;
    if zip.len() > 4096 {
        return Err("Too many archive entries".into());
    }
    let mut files = Files::new();
    let mut names = BTreeSet::new();
    let mut total = 0;
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(error)?;
        if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err("Mod archive contains a symbolic link".into());
        }
        let name = relative(entry.name().trim_end_matches(['/', '\\']))?;
        if !names.insert(name.to_lowercase()) {
            return Err("Duplicate mod archive path".into());
        }
        if entry.is_dir() {
            continue;
        }
        total += entry.size();
        if entry.size() > LIMIT || total > TOTAL {
            return Err("Extracted mod exceeds size limit".into());
        }
        let mut bytes = Vec::new();
        entry
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(error)?;
        if bytes.len() as u64 > LIMIT {
            return Err("Mod member too large".into());
        }
        files.insert(name, bytes);
    }
    if let Some(sums) = files.get("CHECKSUMS") {
        let mut checks = BTreeMap::new();
        for line in std::str::from_utf8(sums).map_err(error)?.lines() {
            let (sha, name) = line
                .split_once(char::is_whitespace)
                .ok_or("Invalid mod checksum row")?;
            let name = relative(name.trim().trim_start_matches('*'))?;
            if sha.len() != 64
                || !sha.bytes().all(|c| c.is_ascii_hexdigit())
                || checks.insert(name, sha.to_ascii_lowercase()).is_some()
            {
                return Err("Invalid mod checksums".into());
            }
        }
        for (name, bytes) in &files {
            if name != "CHECKSUMS"
                && name != "CHECKSUMS.asc"
                && checks.get(name) != Some(&hash(bytes))
            {
                return Err(format!("Mod checksum mismatch: {name}"));
            }
        }
        if checks.keys().any(|name| !files.contains_key(name)) {
            return Err("Mod checksum names a missing member".into());
        }
    }
    Ok(files)
}
fn target_ok(name: &str) -> bool {
    (name.starts_with("ChaosGame/") || name.starts_with("Engine/Config/"))
        && !name.to_ascii_lowercase().ends_with(".exe")
        && !name.to_ascii_lowercase().ends_with(".dll")
}
fn ini_backup(original: &[u8], patch: &[u8]) -> Result<Vec<u8>> {
    // Tempest's ini-backup contains replaced scalar keys, not a full-file copy.
    // Decode BOM-marked text; compatible scalar-key backups are UTF-8.
    let decode = |bytes: &[u8]| -> Result<String> {
        if bytes.starts_with(&[255, 254]) || bytes.starts_with(&[254, 255]) {
            let little = bytes[0] == 255;
            if !bytes.len().is_multiple_of(2) {
                return Err("Invalid UTF-16 INI".into());
            }
            String::from_utf16(
                &bytes[2..]
                    .chunks_exact(2)
                    .map(|b| {
                        if little {
                            u16::from_le_bytes([b[0], b[1]])
                        } else {
                            u16::from_be_bytes([b[0], b[1]])
                        }
                    })
                    .collect::<Vec<_>>(),
            )
            .map_err(error)
        } else {
            String::from_utf8(
                bytes
                    .strip_prefix(&[239, 187, 191])
                    .unwrap_or(bytes)
                    .to_vec(),
            )
            .map_err(error)
        }
    };
    let original = decode(original)?;
    let patch = decode(patch)?;
    let mut section = String::new();
    let mut sections: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for line in patch.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            section = line.to_owned();
            continue;
        }
        if line.starts_with(['+', '-', '!', ';', '#']) {
            continue;
        }
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let mut in_section = false;
        for old in original.lines() {
            let old_trim = old.trim();
            if old_trim.starts_with('[') {
                in_section = old_trim.eq_ignore_ascii_case(&section);
                continue;
            }
            if in_section
                && old_trim.split_once('=').is_some_and(|(k, _)| {
                    k.trim()
                        .trim_start_matches(['+', '-', '!'])
                        .eq_ignore_ascii_case(key.trim())
                })
            {
                sections
                    .entry(section.to_ascii_lowercase())
                    .or_default()
                    .entry(key.trim().to_ascii_lowercase())
                    .or_insert_with(|| old.to_owned());
                break;
            }
        }
    }
    let mut output = String::new();
    for (section, lines) in sections {
        output.push_str(&format!("{section}\n"));
        for line in lines.values() {
            output.push_str(line);
            output.push('\n');
        }
    }
    Ok(output.into_bytes())
}
fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .to_ascii_lowercase()
}
fn protect_target(root: &Path, records: &[Value], name: &str) -> Result<()> {
    for record in records {
        for path in ["InstalledFiles", "OwnedFiles"]
            .iter()
            .flat_map(|key| record[key].as_array().into_iter().flatten())
            .filter_map(Value::as_str)
        {
            let path = Path::new(path);
            let owned = if path.is_absolute() {
                path.to_owned()
            } else {
                root.join(relative(&path.to_string_lossy())?)
            };
            if path_key(&owned) == path_key(&root.join(name)) {
                return Err("Another registered mod owns this game file".into());
            }
        }
    }
    let active = root.join(".watchcat-localization/active.json");
    if active.exists() {
        let receipt: Value =
            serde_json::from_slice(&read(&active, 4 * 1024 * 1024)?).map_err(error)?;
        if receipt["applied"] == true
            && receipt["files"].as_array().into_iter().flatten().any(|f| {
                f["target"]
                    .as_str()
                    .is_some_and(|p| path_key(Path::new(p)) == path_key(&root.join(name)))
            })
        {
            return Err("Restore managed localization before replacing this game file".into());
        }
    }
    Ok(())
}
struct FilePolicy {
    replace: bool,
    legacy: bool,
}
#[derive(Serialize, Deserialize)]
struct IniState {
    path: String,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
}
fn state_path(id: &str, v1: bool) -> String {
    format!(
        ".tempest/v{}/mods/{id}/paladinscat-ini.json",
        if v1 { 1 } else { 2 }
    )
}
fn remember_ini(
    changes: &mut Files,
    id: &str,
    v1: bool,
    path: &str,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
) -> Result<()> {
    let name = state_path(id, v1);
    let mut states: Vec<IniState> = changes
        .get(&name)
        .map(|b| serde_json::from_slice(b))
        .transpose()
        .map_err(error)?
        .unwrap_or_default();
    states.push(IniState {
        path: path.into(),
        before,
        after,
    });
    changes.insert(name, serde_json::to_vec(&states).map_err(error)?);
    Ok(())
}
fn add_game_file(
    root: &Path,
    records: &[Value],
    changes: &mut Files,
    id: &str,
    name: &str,
    bytes: Vec<u8>,
    policy: FilePolicy,
) -> Result<()> {
    let name = relative(name)?;
    if !target_ok(&name) {
        return Err("Mod file target is outside game content/configuration".into());
    }
    protect_target(root, records, &name)?;
    let FilePolicy {
        replace,
        legacy: v1,
    } = policy;
    let target = safe(root, &name)?;
    let before = optional(&target)?;
    let ini = name.to_ascii_lowercase().ends_with(".ini");
    if before.is_some() && !replace && !ini {
        return Err("Enable Replace existing game files to import this mod".into());
    }
    let snapshot = if v1 {
        format!(
            ".tempest/v1/mods/{id}/files/{}",
            target.file_name().unwrap().to_string_lossy()
        )
    } else {
        format!(".tempest/v2/mods/{id}/files/{name}")
    };
    changes.insert(snapshot, bytes.clone());
    if let Some(before) = before.as_ref() {
        let backup = if v1 {
            format!(
                ".tempest/v1/backup/{}",
                target.file_name().unwrap().to_string_lossy()
            )
        } else if ini {
            format!(".tempest/v2/ini-backup/{id}/{name}")
        } else {
            format!(".tempest/v2/backup/{name}")
        };
        if !safe(root, &backup)?.exists() {
            let backup_bytes = if ini {
                ini_backup(before, &bytes)?
            } else {
                before.clone()
            };
            if !backup_bytes.is_empty() {
                changes.insert(backup, backup_bytes);
            }
        }
    }
    let after = if ini {
        patch_ini(before.as_deref().unwrap_or(&[]), &bytes)?
    } else {
        bytes
    };
    if ini {
        remember_ini(changes, id, v1, &name, before, after.clone())?;
    }
    changes.insert(name, after);
    Ok(())
}
pub fn import(
    game: &Path,
    package: &Path,
    replace: bool,
    allow_native: bool,
) -> Result<Vec<ModInfo>> {
    import_with(
        game,
        package,
        replace,
        allow_native,
        &upk_patcher::is_game_running,
        None,
    )
}
pub(crate) fn import_with(
    game: &Path,
    package: &Path,
    replace: bool,
    allow_native: bool,
    running: &dyn Fn() -> bool,
    fail_after: Option<usize>,
) -> Result<Vec<ModInfo>> {
    let (root, _lock) = engine::acquire(game, running)?;
    if !game_discovery::is_game_root(&root) {
        return Err("Select a valid game folder".into());
    }
    if engine::recovery_pending(&root) || crate::ime::recovery_pending(&root) {
        return Err("Recover pending game-file changes first".into());
    }
    if recovery_pending(&root) {
        return Err("Recover the interrupted mod import first".into());
    }
    recover_inner(&root)?;
    let mut records = records(&root)?;
    let mut changes = Files::new();
    let extension = package
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (id, name, version, authors, kind, installed, readme) = if extension == "zip"
        || extension == "tempest"
    {
        let files = archive(package)?;
        let manifest: toml::Value = toml::from_str(
            std::str::from_utf8(
                files
                    .get("manifest.toml")
                    .ok_or("Mod archive has no manifest.toml")?,
            )
            .map_err(error)?,
        )
        .map_err(error)?;
        let info = manifest
            .get("mod")
            .ok_or("Missing [mod] manifest section")?;
        let id = info
            .get("id")
            .and_then(toml::Value::as_str)
            .ok_or("Missing mod id")?
            .to_owned();
        if !launch::mod_id(&id) || launch::family(&id) {
            return Err("Invalid or managed mod ID".into());
        }
        if safe(&root, &format!(".tempest/v2/mods/{id}"))?.exists() {
            return Err("A mod with this ID is already installed".into());
        }
        let name = info
            .get("name")
            .and_then(toml::Value::as_str)
            .unwrap_or(&id)
            .to_owned();
        if launch::family(&name) {
            return Err("Use the managed Localization or Performance controls for this mod".into());
        }
        let version = info
            .get("version")
            .and_then(toml::Value::as_str)
            .unwrap_or("1.0.0")
            .to_owned();
        let authors=info.get("authors").and_then(toml::Value::as_array).into_iter().flatten().filter_map(|a|a.as_str().map(|s|json!({"Name":s})).or_else(||a.get("name").and_then(toml::Value::as_str).map(|name|json!({"Name":name,"Link":a.get("link").and_then(toml::Value::as_str).unwrap_or("")})))).collect::<Vec<_>>();
        let readme = info
            .get("readme")
            .and_then(toml::Value::as_str)
            .unwrap_or("")
            .to_owned();
        let readme = if readme.is_empty() {
            readme
        } else {
            relative(&readme)?
        };
        let mut installed = Vec::new();
        for (path, bytes) in files {
            if path.eq_ignore_ascii_case("paladinscat-ini.json") {
                return Err("Mod archive uses a reserved launcher state filename".into());
            }
            if let Some(target) = path.strip_prefix("files/") {
                add_game_file(
                    &root,
                    &records,
                    &mut changes,
                    &id,
                    target,
                    bytes,
                    FilePolicy {
                        replace,
                        legacy: false,
                    },
                )?;
                installed.push(root.join(target));
            } else {
                if path.to_ascii_lowercase().starts_with("dlls/")
                    && path.to_ascii_lowercase().ends_with(".dll")
                {
                    if !allow_native {
                        return Err("Allow unverified DLL mods to import this package".into());
                    }
                    if launch::family(Path::new(&path).file_name().unwrap().to_str().unwrap_or(""))
                    {
                        return Err(
                            "Use the managed Localization or Performance controls for this DLL"
                                .into(),
                        );
                    }
                    let pe = bytes
                        .get(0x3c..0x40)
                        .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
                        .ok_or("Invalid mod DLL")?;
                    if bytes.get(..2) != Some(b"MZ")
                        || bytes.get(pe..pe.saturating_add(6)) != Some(b"PE\0\0\x64\x86")
                        || bytes.get(pe.saturating_add(24)..pe.saturating_add(26))
                            != Some(&[0x0b, 0x02])
                    {
                        return Err("Only Windows x64 mod DLLs are supported".into());
                    }
                }
                changes.insert(format!(".tempest/v2/mods/{id}/{path}"), bytes);
            }
        }
        (
            id,
            name,
            version,
            authors,
            "V2".to_owned(),
            installed,
            readme,
        )
    } else if extension == "upk" || extension == "pck" {
        let bytes = read(package, LIMIT)?;
        if bytes.is_empty() {
            return Err("Mod file is empty".into());
        }
        let name = relative(
            package
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or("Invalid mod filename")?,
        )?;
        if launch::family(&name) {
            return Err("Use the managed Localization or Performance controls for this mod".into());
        }
        let upper = name.to_ascii_uppercase();
        let voice = upper.contains("_VOX") || upper.contains("_VGS");
        let asset = upper.contains("_SF") || upper.contains("_PC");
        let kind = if voice {
            "Voice"
        } else if asset {
            "Asset"
        } else {
            "NativePackage"
        }
        .to_owned();
        let id = format!("pcmod-{}", &hash(&bytes)[..24]);
        if safe(&root, &format!(".tempest/v1/mods/{id}"))?.exists() {
            return Err("A mod with this ID is already installed".into());
        }
        let target = format!(
            "ChaosGame/CookedPCConsole/{}{name}",
            if voice { "English(US)/" } else { "" }
        );
        add_game_file(
            &root,
            &records,
            &mut changes,
            &id,
            &target,
            bytes,
            FilePolicy {
                replace,
                legacy: true,
            },
        )?;
        let ini = "ChaosGame/Config/DefaultEngine.ini";
        if !upper.contains("_SF") && !upper.contains("WWB") && safe(&root, ini)?.exists() {
            protect_target(&root, &records, ini)?;
            let original = read(&safe(&root, ini)?, LIMIT)?;
            let stem = Path::new(&name).file_stem().unwrap().to_string_lossy();
            let after = patch_ini(
                &original,
                format!("[Engine.ScriptPackages]\n+NativePackages={stem}\n").as_bytes(),
            )?;
            remember_ini(&mut changes, &id, true, ini, Some(original), after.clone())?;
            changes.insert(ini.into(), after);
        }
        (
            id,
            name,
            String::new(),
            Vec::new(),
            kind,
            vec![root.join(target)],
            String::new(),
        )
    } else {
        return Err("Choose a Tempest .tempest, .zip, .upk or .pck mod".into());
    };
    if records
        .iter()
        .any(|r| text(r, "Id").eq_ignore_ascii_case(&id))
    {
        return Err("A mod with this ID is already registered".into());
    }
    let author = authors
        .iter()
        .filter_map(|a| a["Name"].as_str())
        .collect::<Vec<_>>()
        .join(", ");
    // Tempest SaveMetadata persists installation-relative paths, which retain
    // ownership when an installation or Steam library is moved.
    let installed = installed
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .map_err(error)
        })
        .collect::<Result<Vec<_>>>()?;
    records.push(json!({"Id":id,"Name":name,"Version":version,"Author":if author.is_empty(){"Unknown"}else{&author},"Authors":authors,"Kind":kind,"Enabled":true,"OriginalPath":package,"InstalledFiles":installed,"Readme":readme}));
    changes.insert(
        REGISTRY.into(),
        serde_json::to_vec_pretty(&records).map_err(error)?,
    );
    if running() {
        return Err("Close Paladins before changing game files".into());
    }
    transact(&root, changes, fail_after)?;
    list(&root)
}

/// Change a registered mod without importing another copy of its package.
pub fn set_enabled(game: &Path, id: &str, enabled: bool) -> Result<Vec<ModInfo>> {
    change_with(game, id, Some(enabled), &upk_patcher::is_game_running, None)
}
pub fn remove(game: &Path, id: &str) -> Result<Vec<ModInfo>> {
    change_with(game, id, None, &upk_patcher::is_game_running, None)
}

fn game_relative(root: &Path, path: &str) -> Result<String> {
    let path = path.replace('\\', "/");
    let path = path.trim_start_matches("//?/");
    let root_key = path_key(root) + "/";
    let name = if path_key(Path::new(path)).starts_with(&root_key) {
        &path[root_key.len()..]
    } else {
        path
    };
    let name = relative(name)?;
    if !target_ok(&name) {
        return Err("Registered mod target is outside game content/configuration".into());
    }
    Ok(name)
}
fn tree_files(root: &Path, name: &str, depth: usize, files: &mut Vec<String>) -> Result<()> {
    if depth > 24 || files.len() > 4096 {
        return Err("Mod directory exceeds supported size/depth".into());
    }
    let path = safe(root, name)?;
    if !path.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(error)? {
        let entry = entry.map_err(error)?;
        let name = format!(
            "{name}/{}",
            entry.file_name().to_str().ok_or("Invalid mod filename")?
        );
        let path = safe(root, &name)?;
        if path.is_dir() {
            tree_files(root, &name, depth + 1, files)?;
        } else if path.is_file() {
            files.push(name);
        } else {
            return Err("Unsupported mod directory member".into());
        }
    }
    Ok(())
}
type IniGroups = BTreeMap<(String, String), Vec<String>>;
fn ini_groups(text: &str) -> IniGroups {
    let mut groups = IniGroups::new();
    let mut section = String::new();
    for line in text.lines() {
        let s = line.trim();
        if s.starts_with('[') && s.ends_with(']') {
            section = s[1..s.len() - 1].trim().to_ascii_lowercase();
        } else if let Some((key, _)) = ini_entry(line) {
            groups
                .entry((section.clone(), key.to_ascii_lowercase()))
                .or_default()
                .push(line.to_owned());
        }
    }
    groups
}
fn same_ini(a: &str, b: &str) -> bool {
    match (ini_entry(a), ini_entry(b)) {
        (Some((ak, av)), Some((bk, bv))) => {
            ak.eq_ignore_ascii_case(bk) && av.eq_ignore_ascii_case(bv)
        }
        _ => false,
    }
}
/// Restore only entries changed by this mod. Unrelated edits and other array
/// additions survive; a conflicting scalar edit blocks the whole transaction.
fn restore_ini(current: &[u8], before: &[u8], after: &[u8]) -> Result<Vec<u8>> {
    if current == after {
        return Ok(before.to_vec());
    }
    let (current_text, encoding) = decode_ini(current)?;
    let newline = if current_text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let previous = ini_groups(&decode_ini(before)?.0);
    let applied = ini_groups(&decode_ini(after)?.0);
    let empty = Vec::new();
    let mut lines = current_text.lines().map(str::to_owned).collect::<Vec<_>>();
    let keys = previous
        .keys()
        .chain(applied.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for (section, key) in keys {
        let old = previous
            .get(&(section.clone(), key.clone()))
            .unwrap_or(&empty);
        let new = applied
            .get(&(section.clone(), key.clone()))
            .unwrap_or(&empty);
        if old == new {
            continue;
        }
        let mut active = String::new();
        let indices = lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| {
                let s = line.trim();
                if s.starts_with('[') && s.ends_with(']') {
                    active = s[1..s.len() - 1].trim().to_ascii_lowercase();
                }
                (active == section
                    && ini_entry(line).is_some_and(|(k, _)| k.eq_ignore_ascii_case(&key)))
                .then_some(i)
            })
            .collect::<Vec<_>>();
        let array = old
            .iter()
            .chain(new)
            .all(|l| l.trim().starts_with(['+', '.']));
        let (remove, restore) = if array {
            let added = new
                .iter()
                .filter(|l| !old.iter().any(|o| same_ini(o, l)))
                .collect::<Vec<_>>();
            if added
                .iter()
                .any(|l| !indices.iter().any(|i| same_ini(&lines[*i], l)))
            {
                return Err(
                    "A mod INI entry changed; preserve it before disabling this mod".into(),
                );
            }
            (
                indices
                    .iter()
                    .copied()
                    .filter(|i| added.iter().any(|l| same_ini(&lines[*i], l)))
                    .collect::<Vec<_>>(),
                old.iter()
                    .filter(|l| {
                        !new.iter().any(|n| same_ini(n, l))
                            && !indices.iter().any(|i| same_ini(&lines[*i], l))
                    })
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        } else {
            if indices.len() != new.len()
                || indices
                    .iter()
                    .zip(new)
                    .any(|(i, l)| !same_ini(&lines[*i], l))
            {
                return Err(
                    "A mod INI entry changed; preserve it before disabling this mod".into(),
                );
            }
            (indices, old.clone())
        };
        for i in remove.into_iter().rev() {
            lines.remove(i);
        }
        if !restore.is_empty() {
            let header = format!("[{section}]");
            let start = lines
                .iter()
                .position(|l| l.trim().eq_ignore_ascii_case(&header))
                .unwrap_or_else(|| {
                    lines.push(header);
                    lines.len() - 1
                });
            let end = (start + 1..lines.len())
                .find(|i| lines[*i].trim().starts_with('['))
                .unwrap_or(lines.len());
            lines.splice(end..end, restore);
        }
    }
    Ok(encode_ini(lines.join(newline) + newline, encoding))
}
fn legacy_ini_before(current: &[u8], patch: &[u8], backup: &[u8]) -> Result<Vec<u8>> {
    let mut inverse = String::new();
    let mut section = String::new();
    let saved = ini_groups(&decode_ini(backup)?.0);
    for line in decode_ini(patch)?.0.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            inverse.push_str(&format!("{line}\n"));
        } else if let Some((key, value)) = ini_entry(line) {
            if line.starts_with(['-', '!']) {
                return Err(
                    "Original INI snapshot is missing for this older mod; restore its original settings before removal"
                        .into(),
                );
            }
            if line.starts_with('+') {
                inverse.push_str(&format!("-{key}={value}\n"));
            } else if let Some(old) = saved.get(&(section.clone(), key.to_ascii_lowercase())) {
                for entry in old {
                    inverse.push_str(entry);
                    inverse.push('\n');
                }
            } else {
                inverse.push_str(&format!("!{key}=\n"));
            }
        }
    }
    patch_ini(current, inverse.as_bytes())
}

pub(crate) fn change_with(
    game: &Path,
    id: &str,
    enabled: Option<bool>,
    running: &dyn Fn() -> bool,
    fail_after: Option<usize>,
) -> Result<Vec<ModInfo>> {
    let (root, _lock) = engine::acquire(game, running)?;
    if !game_discovery::is_game_root(&root) {
        return Err("Select a valid game folder".into());
    }
    if engine::recovery_pending(&root) || crate::ime::recovery_pending(&root) {
        return Err("Recover pending game-file changes first".into());
    }
    recover_inner(&root)?;
    if !launch::mod_id(id) {
        return Err("Invalid mod ID".into());
    }
    let mut registry = records(&root)?;
    let index = registry
        .iter()
        .position(|r| text(r, "Id").eq_ignore_ascii_case(id))
        .ok_or("Mod is no longer installed; refresh the list")?;
    let record = &registry[index];
    if launch::family(id) || launch::family(text(record, "Name")) {
        return Err("Use the managed Localization or Performance controls for this mod".into());
    }
    let v1 = match text(record, "Kind") {
        "V2" => false,
        "Voice" | "Asset" | "NativePackage" => true,
        _ => return Err("Unknown mod format; preserve its files and registry".into()),
    };
    let active = record["Enabled"].as_bool().unwrap_or(true);
    if enabled == Some(active) {
        return list(&root);
    }
    let id = text(record, "Id");
    let mod_dir = format!(".tempest/v{}/mods/{id}", if v1 { 1 } else { 2 });
    let mut stored = Vec::new();
    tree_files(&root, &mod_dir, 0, &mut stored)?;
    if stored.iter().any(|p| {
        Path::new(p)
            .file_name()
            .and_then(|p| p.to_str())
            .is_some_and(launch::family)
    }) {
        return Err("Use the managed Localization or Performance controls for this mod".into());
    }
    if enabled == Some(true) && !safe(&root, &mod_dir)?.is_dir() {
        return Err("Mod snapshot is missing; reimport its package".into());
    }
    let mut targets = record["InstalledFiles"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| game_relative(&root, p.as_str().ok_or("Invalid registered mod file")?))
        .collect::<Result<BTreeSet<_>>>()?;
    if targets.len() > 4096 {
        return Err("Too many mod files".into());
    }
    let state_name = state_path(id, v1);
    let states: Vec<IniState> = optional(&safe(&root, &state_name)?)?
        .map(|b| serde_json::from_slice(&b).map_err(error))
        .transpose()?
        .unwrap_or_default();
    for state in &states {
        targets.insert(game_relative(&root, &state.path)?);
    }
    let others = registry
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != index)
        .map(|(_, r)| r.clone())
        .collect::<Vec<_>>();
    let mut changes = BTreeMap::new();
    let mut next_states = Vec::new();
    let mut backups = BTreeSet::new();
    for name in &targets {
        protect_target(&root, &others, name)?;
        let filename = Path::new(name).file_name().unwrap().to_string_lossy();
        let ini = name.to_ascii_lowercase().ends_with(".ini");
        let snapshot = if v1 {
            format!("{mod_dir}/files/{filename}")
        } else {
            format!("{mod_dir}/files/{name}")
        };
        let backup = if v1 {
            format!(".tempest/v1/backup/{filename}")
        } else if ini {
            format!(".tempest/v2/ini-backup/{id}/{name}")
        } else {
            format!(".tempest/v2/backup/{name}")
        };
        // V1 NativePackages registration is shared INI state, not a V1 file backup.
        if !(v1 && ini) {
            backups.insert(backup.clone());
        }
        if !active && enabled.is_none() {
            continue;
        }
        let current = optional(&safe(&root, name)?)?;
        let state = states.iter().find(|s| s.path.eq_ignore_ascii_case(name));
        let payload = if v1 && ini {
            let mut patch = String::from("[Engine.ScriptPackages]\n");
            for target in &targets {
                let upper = target.to_ascii_uppercase();
                if !upper.ends_with(".INI") && !upper.contains("_SF") && !upper.contains("WWB") {
                    patch.push_str(&format!(
                        "+NativePackages={}\n",
                        Path::new(target).file_stem().unwrap().to_string_lossy()
                    ));
                }
            }
            patch.into_bytes()
        } else {
            read(&safe(&root, &snapshot)?, LIMIT)
                .map_err(|_| "Mod snapshot is missing; reimport its package".to_string())?
        };
        let restored = optional(&safe(&root, &backup)?)?;
        if enabled == Some(true) {
            if ini {
                let after = patch_ini(current.as_deref().unwrap_or(&[]), &payload)?;
                if !v1 {
                    changes.insert(
                        backup,
                        Some(ini_backup(current.as_deref().unwrap_or(&[]), &payload)?),
                    );
                }
                next_states.push(IniState {
                    path: name.clone(),
                    before: current,
                    after: after.clone(),
                });
                changes.insert(name.clone(), Some(after));
            } else {
                if current != restored {
                    return Err(
                        "A disabled mod's game file changed; preserve it before enabling".into(),
                    );
                }
                changes.insert(name.clone(), Some(payload));
            }
        } else if ini {
            let current =
                current.ok_or("Mod INI is missing; preserve the snapshot before removing")?;
            let (before, after) = if let Some(state) = state {
                (state.before.clone(), state.after.clone())
            } else {
                let before =
                    legacy_ini_before(&current, &payload, restored.as_deref().unwrap_or(&[]))?;
                let after = patch_ini(&before, &payload)?;
                (Some(before), after)
            };
            let result = restore_ini(&current, before.as_deref().unwrap_or(&[]), &after)?;
            changes.insert(
                name.clone(),
                if before.is_none() && current == after {
                    None
                } else {
                    Some(result)
                },
            );
        } else {
            if current.as_deref() != Some(payload.as_slice()) {
                return Err(
                    "A mod game file changed; preserve it before disabling or removing".into(),
                );
            }
            changes.insert(name.clone(), restored);
        }
    }
    // Old Tempest V1 entries predate the saved INI state.
    if v1 && states.is_empty() && !(enabled.is_none() && !active) {
        let name = "ChaosGame/Config/DefaultEngine.ini";
        let mut patch = String::from("[Engine.ScriptPackages]\n");
        for target in &targets {
            let upper = target.to_ascii_uppercase();
            if !upper.contains("_SF") && !upper.contains("WWB") {
                patch.push_str(&format!(
                    "{}NativePackages={}\n",
                    if enabled == Some(true) { "+" } else { "-" },
                    Path::new(target).file_stem().unwrap().to_string_lossy()
                ));
            }
        }
        if patch.lines().count() > 1
            && let Some(current) = optional(&safe(&root, name)?)?
        {
            protect_target(&root, &others, name)?;
            let after = patch_ini(&current, patch.as_bytes())?;
            if enabled == Some(true) {
                next_states.push(IniState {
                    path: name.into(),
                    before: Some(current),
                    after: after.clone(),
                });
            }
            changes.insert(name.into(), Some(after));
        }
    }
    let mut cleanup = Vec::new();
    if let Some(enabled) = enabled {
        registry[index]["Enabled"] = Value::Bool(enabled);
        if enabled {
            changes.insert(
                state_name,
                Some(serde_json::to_vec(&next_states).map_err(error)?),
            );
        }
    } else {
        let ini_dir = format!(".tempest/v2/ini-backup/{id}");
        tree_files(&root, &ini_dir, 0, &mut stored)?;
        for name in stored.into_iter().chain(backups) {
            changes.insert(name, None);
        }
        cleanup.push(mod_dir);
        if !v1 {
            cleanup.push(ini_dir);
        }
        registry.remove(index);
    }
    changes.insert(
        REGISTRY.into(),
        Some(serde_json::to_vec_pretty(&registry).map_err(error)?),
    );
    if running() {
        return Err("Close Paladins before changing game files".into());
    }
    transact_changes(&root, changes, fail_after)?;
    for name in cleanup {
        let mut remaining = Vec::new();
        if tree_files(&root, &name, 0, &mut remaining).is_ok()
            && remaining.is_empty()
            && safe(&root, &name)?.exists()
            && let Err(e) = cache::recycle_owned(&root, &[safe(&root, &name)?])
        {
            cache::cleanup_warning(format!("Mod folder cleanup pending: {e}"));
        }
    }
    list(&root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture {
        root: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/mod-import-tests")
                .join(format!(
                    "{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ))
                .join("game");
            fs::create_dir_all(&root).unwrap();
            let f = Self {
                root: root.canonicalize().unwrap(),
            };
            f.put(
                "Binaries/Win64/Paladins.exe",
                b"Owned fixture; never executed",
            );
            fs::create_dir_all(f.root.join("Engine")).unwrap();
            f.put(
                "ChaosGame/Config/DefaultEngine.ini",
                b"[Engine.ScriptPackages]\r\n+NativePackages=Stock\r\n",
            );
            f
        }
        fn put(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let p = self.root.join(name);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, bytes).unwrap();
            p
        }
        fn zip(&self, items: &[(&str, &[u8])]) -> PathBuf {
            let p = self.root.join(format!(
                "input-{}.tempest",
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut zip = zip::ZipWriter::new(File::create(&p).unwrap());
            for (name, bytes) in items {
                zip.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(bytes).unwrap();
            }
            zip.finish().unwrap();
            p
        }
        fn add(&self, p: &Path, replace: bool, native: bool) -> Result<Vec<ModInfo>> {
            import_with(&self.root, p, replace, native, &|| false, None)
        }
    }
    const MANIFEST:&[u8]=b"[mod]\nid='fixture-mod'\nname='Fixture mod'\nversion='1.2.3'\nauthors=['Fixture author']\nreadme='README.md'\n";
    fn control(f: &Fixture, id: &str, enabled: Option<bool>) -> Result<Vec<ModInfo>> {
        change_with(&f.root, id, enabled, &|| false, None)
    }
    #[test]
    fn v2_disable_reenable_remove_restores_originals_and_removes_new_files_and_snapshots() {
        let f = Fixture::new();
        let stock = f.put("ChaosGame/CookedPCConsole/stock_SF.upk", b"original");
        let added = "ChaosGame/CookedPCConsole/added_SF.upk";
        let zip = f.zip(&[
            ("manifest.toml", MANIFEST),
            ("files/ChaosGame/CookedPCConsole/stock_SF.upk", b"mod"),
            ("files/ChaosGame/CookedPCConsole/added_SF.upk", b"added"),
        ]);
        f.add(&zip, true, false).unwrap();
        assert!(!control(&f, "fixture-mod", Some(false)).unwrap()[0].enabled);
        assert_eq!(fs::read(&stock).unwrap(), b"original");
        assert!(!f.root.join(added).exists());
        assert!(f.root.join(".tempest/v2/mods/fixture-mod/files").is_dir());
        assert!(control(&f, "fixture-mod", Some(true)).unwrap()[0].enabled);
        assert_eq!(fs::read(&stock).unwrap(), b"mod");
        assert_eq!(fs::read(f.root.join(added)).unwrap(), b"added");
        assert!(control(&f, "fixture-mod", None).unwrap().is_empty());
        assert_eq!(fs::read(stock).unwrap(), b"original");
        assert!(!f.root.join(added).exists());
        assert!(!f.root.join(".tempest/v2/mods/fixture-mod").exists());
        assert!(
            !f.root
                .join(".tempest/v2/backup/ChaosGame/CookedPCConsole/stock_SF.upk")
                .exists()
        );
    }
    #[test]
    fn removing_disabled_mod_preserves_subsequent_game_changes_and_registry_metadata() {
        let f = Fixture::new();
        let target = f.put("ChaosGame/CookedPCConsole/stock_SF.upk", b"original");
        let zip = f.zip(&[
            ("manifest.toml", MANIFEST),
            ("files/ChaosGame/CookedPCConsole/stock_SF.upk", b"mod"),
        ]);
        f.add(&zip, true, false).unwrap();
        let mut entries = records(&f.root).unwrap();
        entries[0]["Future"] = json!({"preserved":true});
        let other = json!({"Id":"other","Kind":"V2","Enabled":false,"Future":42});
        entries.push(other.clone());
        f.put(REGISTRY, &serde_json::to_vec(&entries).unwrap());
        control(&f, "fixture-mod", Some(false)).unwrap();
        assert_eq!(records(&f.root).unwrap()[0]["Future"]["preserved"], true);
        fs::write(&target, b"user update").unwrap();
        assert!(
            control(&f, "fixture-mod", Some(true))
                .unwrap_err()
                .contains("changed")
        );
        assert_eq!(control(&f, "fixture-mod", None).unwrap().len(), 1);
        assert_eq!(fs::read(target).unwrap(), b"user update");
        assert_eq!(records(&f.root).unwrap(), vec![other]);
    }
    #[test]
    fn v1_voice_and_shared_native_packages_round_trip_without_clobbering_other_entries() {
        let f = Fixture::new();
        let target = f.put(
            "ChaosGame/CookedPCConsole/English(US)/Hero_VOX.pck",
            b"stock voice",
        );
        let package = f.put("Hero_VOX.pck", b"mod voice");
        let id = f.add(&package, true, false).unwrap()[0].id.clone();
        let ini = "ChaosGame/Config/DefaultEngine.ini";
        let mut bytes = fs::read(f.root.join(ini)).unwrap();
        bytes.extend_from_slice(b"+NativePackages=OtherMod\r\n[UserSettings]\r\nKeep=1\r\n");
        f.put(ini, &bytes);
        control(&f, &id, Some(false)).unwrap();
        let text = fs::read_to_string(f.root.join(ini)).unwrap();
        assert!(!text.contains("Hero_VOX"));
        assert!(text.contains("OtherMod") && text.contains("Keep=1"));
        assert_eq!(fs::read(&target).unwrap(), b"stock voice");
        control(&f, &id, Some(true)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"mod voice");
        control(&f, &id, None).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"stock voice");
        let text = fs::read_to_string(f.root.join(ini)).unwrap();
        assert!(!text.contains("Hero_VOX") && text.contains("OtherMod") && text.contains("Keep=1"));
    }
    #[test]
    fn ini_scalar_array_removal_clear_utf16_and_external_conflict_are_reversible() {
        let f = Fixture::new();
        let text = "[Settings]\r\nFlag=old\r\n+Items=Keep\r\n+Items=Removed\r\n+Clear=Old\r\n";
        let original = encode_ini(text.into(), 1);
        let target = f.put("ChaosGame/Config/Test.ini", &original);
        let patch = b"[Settings]\nFlag=new\n+Items=Added\n-Items=Removed\n!Clear=\n";
        let zip = f.zip(&[
            ("manifest.toml", MANIFEST),
            ("files/ChaosGame/Config/Test.ini", patch),
        ]);
        f.add(&zip, false, false).unwrap();
        let applied = fs::read(&target).unwrap();
        let changed = decode_ini(&applied)
            .unwrap()
            .0
            .replace("Flag=new", "Flag=user");
        fs::write(&target, encode_ini(changed, 1)).unwrap();
        assert!(
            control(&f, "fixture-mod", Some(false))
                .unwrap_err()
                .contains("INI entry changed")
        );
        assert!(list(&f.root).unwrap()[0].enabled);
        fs::write(&target, &applied).unwrap();
        control(&f, "fixture-mod", Some(false)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), original);
        control(&f, "fixture-mod", Some(true)).unwrap();
        control(&f, "fixture-mod", None).unwrap();
        assert_eq!(fs::read(target).unwrap(), original);
    }
    #[test]
    fn tempest_existing_v1_absolute_paths_and_v2_scalar_ini_backups_can_be_controlled() {
        let f = Fixture::new();
        let target = f.put("ChaosGame/CookedPCConsole/Existing_SF.upk", b"mod");
        f.put(".tempest/v1/mods/existing/files/Existing_SF.upk", b"mod");
        f.put(".tempest/v1/backup/Existing_SF.upk", b"stock");
        f.put(
            REGISTRY,
            &serde_json::to_vec(
                &json!([{"Id":"existing","Kind":"Asset","Enabled":true,"InstalledFiles":[target]}]),
            )
            .unwrap(),
        );
        control(&f, "existing", Some(false)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"stock");
        control(&f, "existing", Some(true)).unwrap();
        control(&f, "existing", None).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"stock");
        let f = Fixture::new();
        f.put(
            "ChaosGame/Config/Existing.ini",
            b"[S]\nFlag=new\n+Items=Stock\n+Items=Mod\n",
        );
        f.put(
            ".tempest/v2/mods/existing/files/ChaosGame/Config/Existing.ini",
            b"[S]\nFlag=new\n+Items=Mod\n",
        );
        f.put(
            ".tempest/v2/ini-backup/existing/ChaosGame/Config/Existing.ini",
            b"[S]\nFlag=old\n",
        );
        f.put(REGISTRY,br#"[{"Id":"existing","Kind":"V2","Enabled":true,"InstalledFiles":["ChaosGame/Config/Existing.ini"]}]"#);
        control(&f, "existing", Some(false)).unwrap();
        let ini = fs::read_to_string(f.root.join("ChaosGame/Config/Existing.ini")).unwrap();
        assert!(
            ini.contains("Flag=old") && ini.contains("Items=Stock") && !ini.contains("Items=Mod")
        );
        control(&f, "existing", Some(true)).unwrap();
        control(&f, "existing", None).unwrap();
    }
    #[test]
    fn interrupted_removal_rolls_back_deleted_payloads_backups_registry_and_enabled_state() {
        for fail in 0..4 {
            let f = Fixture::new();
            let target = f.put("ChaosGame/CookedPCConsole/stock_SF.upk", b"stock");
            let zip = f.zip(&[
                ("manifest.toml", MANIFEST),
                ("files/ChaosGame/CookedPCConsole/stock_SF.upk", b"mod"),
            ]);
            f.add(&zip, true, false).unwrap();
            let registry = fs::read(f.root.join(REGISTRY)).unwrap();
            assert!(
                change_with(&f.root, "fixture-mod", None, &|| false, Some(fail))
                    .unwrap_err()
                    .contains("previous files restored")
            );
            assert_eq!(fs::read(&target).unwrap(), b"mod");
            assert_eq!(fs::read(f.root.join(REGISTRY)).unwrap(), registry);
            assert_eq!(
                fs::read(
                    f.root
                        .join(".tempest/v2/backup/ChaosGame/CookedPCConsole/stock_SF.upk")
                )
                .unwrap(),
                b"stock"
            );
            assert!(
                f.root
                    .join(".tempest/v2/mods/fixture-mod/manifest.toml")
                    .exists()
            );
            assert!(!recovery_pending(&f.root));
            control(&f, "fixture-mod", None).unwrap();
            assert_eq!(fs::read(target).unwrap(), b"stock");
        }
    }
    #[test]
    fn state_controls_refuse_running_game_managed_mods_external_changes_and_unsafe_targets() {
        let f = Fixture::new();
        let package = f.put("Fixture_SF.upk", b"mod");
        let id = f.add(&package, false, false).unwrap()[0].id.clone();
        assert!(
            change_with(&f.root, &id, Some(false), &|| true, None)
                .unwrap_err()
                .contains("Close Paladins")
        );
        let registry = fs::read(f.root.join(REGISTRY)).unwrap();
        f.put("ChaosGame/CookedPCConsole/Fixture_SF.upk", b"external edit");
        assert!(control(&f, &id, None).unwrap_err().contains("changed"));
        assert_eq!(fs::read(f.root.join(REGISTRY)).unwrap(), registry);
        let mut entries = records(&f.root).unwrap();
        entries[0]["Name"] = "PaladinsCat IME".into();
        f.put(REGISTRY, &serde_json::to_vec(&entries).unwrap());
        assert!(control(&f, &id, None).unwrap_err().contains("managed"));
        entries[0]["Name"] = "ordinary".into();
        entries[0]["InstalledFiles"] = json!(["../outside"]);
        f.put(REGISTRY, &serde_json::to_vec(&entries).unwrap());
        assert!(control(&f, &id, None).unwrap_err().contains("Unsafe"));
    }
    fn dll() -> Vec<u8> {
        let mut bytes = vec![0; 128];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..70].copy_from_slice(b"PE\0\0\x64\x86");
        bytes[88..90].copy_from_slice(&[0x0b, 0x02]);
        bytes
    }
    #[test]
    fn imports_v2_and_lists_legacy_records_without_dropping_unknown_metadata() {
        let f = Fixture::new();
        let old = json!({"Id":"Old","Name":"Existing mod","Kind":"V2","Enabled":false,"Future":{"keep":true}});
        f.put(REGISTRY, &serde_json::to_vec(&vec![old.clone()]).unwrap());
        let zip = f.zip(&[
            ("manifest.toml", MANIFEST),
            (
                "files/ChaosGame/CookedPCConsole/fixture_SF.upk",
                b"owned payload",
            ),
            ("README.md", b"Fixture readme"),
        ]);
        let mods = f.add(&zip, false, false).unwrap();
        assert_eq!(mods.len(), 2);
        assert!(!mods[0].enabled);
        assert!(mods[1].enabled);
        assert_eq!(mods[1].author, "Fixture author");
        assert_eq!(records(&f.root).unwrap()[0], old);
        assert_eq!(
            records(&f.root).unwrap()[1]["InstalledFiles"][0],
            "ChaosGame/CookedPCConsole/fixture_SF.upk"
        );
        assert_eq!(
            fs::read(f.root.join("ChaosGame/CookedPCConsole/fixture_SF.upk")).unwrap(),
            b"owned payload"
        );
        assert_eq!(
            fs::read(f.root.join(
                ".tempest/v2/mods/fixture-mod/files/ChaosGame/CookedPCConsole/fixture_SF.upk"
            ))
            .unwrap(),
            b"owned payload"
        );
        assert!(!recovery_pending(&f.root));
        assert!(
            f.add(&zip, false, false)
                .unwrap_err()
                .contains("already installed")
        );
    }
    #[test]
    fn legacy_voice_replacement_requires_opt_in_and_retains_pristine_backup() {
        let f = Fixture::new();
        let target = f.put(
            "ChaosGame/CookedPCConsole/English(US)/Hero_VOX.pck",
            b"original",
        );
        let input = f.put("Hero_VOX.pck", b"new voice");
        assert!(
            f.add(&input, false, false)
                .unwrap_err()
                .contains("Replace existing")
        );
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert!(!f.root.join(REGISTRY).exists());
        let mods = f.add(&input, true, false).unwrap();
        assert_eq!(mods[0].kind, "Voice");
        assert_eq!(fs::read(target).unwrap(), b"new voice");
        assert_eq!(
            fs::read(f.root.join(".tempest/v1/backup/Hero_VOX.pck")).unwrap(),
            b"original"
        );
        let ini = fs::read_to_string(f.root.join("ChaosGame/Config/DefaultEngine.ini")).unwrap();
        assert_eq!(ini.matches("+NativePackages=Hero_VOX").count(), 1);
    }
    #[test]
    fn utf16_ini_merge_preserves_encoding_and_uses_tempest_scalar_backup_layout() {
        let f = Fixture::new();
        let original = "[Engine.ScriptPackages]\r\n+NativePackages=Stock\r\n[SystemSettings]\r\nFlag=old\r\nSecond=before\r\n";
        let original = [
            vec![255, 254],
            original.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        ]
        .concat();
        let target = f.put("ChaosGame/Config/DefaultEngine.ini", &original);
        let patch=b"[Engine.ScriptPackages]\n+NativePackages=Added\n+NativePackages=Added\n[SystemSettings]\nFlag=new\nSecond=after\n";
        let zip = f.zip(&[
            ("manifest.toml", MANIFEST),
            ("files/ChaosGame/Config/DefaultEngine.ini", patch),
        ]);
        f.add(&zip, false, false).unwrap();
        let bytes = fs::read(target).unwrap();
        assert!(bytes.starts_with(&[255, 254]));
        let text = String::from_utf16(
            &bytes[2..]
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(text.contains("Flag=new\r\n"));
        assert_eq!(text.matches("+NativePackages=Added").count(), 1);
        assert!(text.contains("+NativePackages=Stock"));
        let backup = fs::read_to_string(
            f.root
                .join(".tempest/v2/ini-backup/fixture-mod/ChaosGame/Config/DefaultEngine.ini"),
        )
        .unwrap();
        assert_eq!(backup.matches("[systemsettings]").count(), 1);
        assert!(backup.contains("Flag=old"));
        assert!(backup.contains("Second=before"));
        assert!(!backup.contains("NativePackages"));
    }
    #[test]
    fn archive_dll_trust_is_explicit_even_for_uppercase_directory_and_bad_dll_is_refused() {
        let f = Fixture::new();
        let bytes = dll();
        let zip = f.zip(&[("manifest.toml", MANIFEST), ("DLLS/fixture.dll", &bytes)]);
        assert!(
            f.add(&zip, false, false)
                .unwrap_err()
                .contains("Allow unverified")
        );
        assert!(!f.root.join(REGISTRY).exists());
        let records = f.add(&zip, false, true).unwrap();
        assert!(records[0].enabled);
        assert!(
            f.root
                .join(".tempest/v2/mods/fixture-mod/dlls/fixture.dll")
                .is_file()
        );
        let f = Fixture::new();
        let zip = f.zip(&[("manifest.toml", MANIFEST), ("dlls/bad.dll", b"not a DLL")]);
        assert!(f.add(&zip, false, true).is_err());
        assert!(!f.root.join(REGISTRY).exists());
    }
    #[test]
    fn checksums_traversal_duplicate_windows_names_and_managed_mods_fail_before_writes() {
        for items in [
            vec![
                ("manifest.toml", MANIFEST),
                ("../escape", b"bad".as_slice()),
            ],
            vec![
                ("manifest.toml", MANIFEST),
                ("README.md", b"one".as_slice()),
                ("readme.md", b"two".as_slice()),
            ],
            vec![
                ("manifest.toml", MANIFEST),
                ("files/ChaosGame/CON.txt", b"bad".as_slice()),
            ],
            vec![
                ("manifest.toml", MANIFEST),
                ("CHECKSUMS", b"wrong  manifest.toml".as_slice()),
            ],
            vec![(
                "manifest.toml",
                b"[mod]\nid='paladins-performance'".as_slice(),
            )],
        ] {
            let f = Fixture::new();
            let zip = f.zip(&items);
            assert!(f.add(&zip, true, true).is_err());
            assert!(!f.root.join(REGISTRY).exists());
            assert!(!f.root.join(TXN).exists());
        }
        let f = Fixture::new();
        let sums = format!(
            "{}  manifest.toml\n{}  README.md\n",
            hash(MANIFEST),
            hash(b"readme")
        );
        let zip = f.zip(&[
            ("manifest.toml", MANIFEST),
            ("README.md", b"readme"),
            ("CHECKSUMS", sums.as_bytes()),
        ]);
        assert!(f.add(&zip, false, false).is_ok());
    }
    #[test]
    fn import_rejects_other_mod_ownership_running_game_and_held_shared_lock() {
        let f = Fixture::new();
        let target = f.put("ChaosGame/CookedPCConsole/fixture_SF.upk", b"other mod");
        f.put(
            REGISTRY,
            &serde_json::to_vec(&json!([{"Id":"other","InstalledFiles":[target]}])).unwrap(),
        );
        let zip = f.zip(&[
            ("manifest.toml", MANIFEST),
            ("files/ChaosGame/CookedPCConsole/fixture_SF.upk", b"ours"),
        ]);
        assert!(
            f.add(&zip, true, false)
                .unwrap_err()
                .contains("Another registered")
        );
        assert!(
            import_with(&f.root, &zip, true, false, &|| true, None)
                .unwrap_err()
                .contains("Close Paladins")
        );
        let (_root, _lease) = engine::acquire(&f.root, &|| false).unwrap();
        assert!(
            f.add(&zip, true, false)
                .unwrap_err()
                .contains("Another patch")
        );
    }
    #[test]
    fn interrupted_import_restores_original_files_registry_and_remains_retryable() {
        let f = Fixture::new();
        let target = f.put("ChaosGame/CookedPCConsole/fixture_SF.upk", b"stock");
        let old = b"[ { \"Id\": \"old\", \"Extra\": 9 } ]";
        f.put(REGISTRY, old);
        let zip = f.zip(&[
            ("manifest.toml", MANIFEST),
            ("files/ChaosGame/CookedPCConsole/fixture_SF.upk", b"ours"),
        ]);
        assert!(
            import_with(&f.root, &zip, true, false, &|| false, Some(3))
                .unwrap_err()
                .contains("previous files restored")
        );
        assert_eq!(fs::read(target).unwrap(), b"stock");
        assert_eq!(fs::read(f.root.join(REGISTRY)).unwrap(), old);
        assert!(!recovery_pending(&f.root));
        assert!(f.add(&zip, true, false).is_ok());
    }

    #[test]
    fn recovery_preserves_changed_files_and_blocks_other_installers_until_resolved() {
        let f = Fixture::new();
        let root = f.root.canonicalize().unwrap();
        let target = f.put("ChaosGame/CookedPCConsole/recover_SF.upk", b"external edit");
        f.put(&format!("{TXN}/old/0"), b"original");
        let j = Journal {
            committed: false,
            changes: vec![Change {
                path: "ChaosGame/CookedPCConsole/recover_SF.upk".into(),
                before: Some(hash(b"original")),
                after: Some(hash(b"imported")),
            }],
            created: Vec::new(),
        };
        save_journal(&root, &j).unwrap();
        assert!(recovery_pending(&root));
        assert!(
            engine::acquire(&root, &|| false)
                .unwrap_err()
                .contains("Recover")
        );
        assert!(recover_inner(&root).unwrap_err().contains("changed"));
        assert_eq!(fs::read(&target).unwrap(), b"external edit");
        fs::write(&target, b"imported").unwrap();
        recover_inner(&root).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"original");
        assert!(!recovery_pending(&root));
    }

    #[test]
    fn incomplete_preparation_without_a_journal_is_recoverable_but_unknown_files_are_preserved() {
        let f = Fixture::new();
        f.put(&format!("{TXN}/journal.next"), b"partial journal");
        fs::create_dir_all(f.root.join(TXN).join("old")).unwrap();
        fs::create_dir_all(f.root.join(TXN).join("new")).unwrap();
        assert!(recovery_pending(&f.root));
        recover_inner(&f.root).unwrap();
        assert!(!recovery_pending(&f.root));
        let foreign = f.put(&format!("{TXN}/foreign"), b"preserve");
        assert!(recover_inner(&f.root).is_err());
        assert_eq!(fs::read(foreign).unwrap(), b"preserve");
    }

    #[test]
    fn legacy_native_registration_cannot_overwrite_a_managed_localization_ini() {
        let f = Fixture::new();
        let ini = f.root.join("ChaosGame/Config/DefaultEngine.ini");
        f.put(
            ".watchcat-localization/active.json",
            &serde_json::to_vec(&json!({"applied":true,"files":[{"target":ini}]})).unwrap(),
        );
        let package = f.put("ConsoleFixture.upk", b"owned native package");
        assert!(
            f.add(&package, true, false)
                .unwrap_err()
                .contains("managed localization")
        );
        assert!(!f.root.join(REGISTRY).exists());
        assert!(
            !f.root
                .join("ChaosGame/CookedPCConsole/ConsoleFixture.upk")
                .exists()
        );
    }
}
