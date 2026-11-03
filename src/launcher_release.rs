//! Direct EXEs and corresponding source from an immutable public source tree.
use paladinscat_launcher::engine;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    process::Command,
};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

type Result<T> = std::result::Result<T, String>;
const BINARIES: [&str; 2] = ["paladinscat-launcher.exe", "paladinscat-launcher-cli.exe"];
const NOTICES: [(&str, &str); 5] = [
    ("LICENSE", "LICENSE"),
    ("NOTICE.md", "NOTICE.md"),
    ("assets/Inter-LICENSE.txt", "licenses/Inter-LICENSE.txt"),
    (
        "licenses/PaladinsCat-mods-MIT.txt",
        "licenses/PaladinsCat-mods-MIT.txt",
    ),
    ("licenses/Lucide-ISC.txt", "licenses/Lucide-ISC.txt"),
];

#[derive(Clone, Serialize)]
struct Artifact {
    file: String,
    bytes: u64,
    sha256: String,
}

#[derive(Clone)]
struct LockedCrate {
    name: String,
    version: String,
    checksum: String,
}

struct VendorCrate {
    package: LockedCrate,
    directory: PathBuf,
    files: BTreeMap<String, String>,
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn artifact(path: &Path) -> Result<Artifact> {
    let mut input = File::open(path).map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0; 131_072];
    loop {
        let count = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        bytes += count as u64;
    }
    Ok(Artifact {
        file: path
            .file_name()
            .and_then(|p| p.to_str())
            .ok_or("Invalid artifact name")?
            .into(),
        bytes,
        sha256: format!("{:x}", digest.finalize()),
    })
}

fn safe_member(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':', '\0', '\n', '\r'])
        && path
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != "..")
}

fn source_path(path: &str) -> bool {
    if !safe_member(path) {
        return false;
    }
    matches!(
        path,
        "Cargo.toml"
            | "Cargo.lock"
            | "rust-toolchain.toml"
            | "build.rs"
            | "LICENSE"
            | "NOTICE.md"
            | "README.md"
            | "CONTRIBUTING.md"
            | "SECURITY.md"
            | ".gitignore"
            | "assets/Inter-Regular.ttf"
            | "assets/Inter-Bold.ttf"
            | "assets/Inter-LICENSE.txt"
            | "assets/paladinscat.png"
            | "licenses/PaladinsCat-mods-MIT.txt"
            | "licenses/Lucide-ISC.txt"
            | "examples/verify_icon.rs"
            | "examples/native_loader_fixture.rs"
            | "examples/updater_fixture.rs"
            | "examples/verify_updater.rs"
            | "examples/verify_release.rs"
            | "docs/architecture.md"
            | "docs/package-versions.md"
            | "docs/self-update.md"
            | "docs/tempest-mods.md"
            | "docs/release-0.3.8.md"
            | "docs/dependency-sources.json"
            | "docs/replay-profiles.md"
            | "docs/download-cache.md"
            | "docs/verification-0.3.6.md"
            | "docs/verification-0.3.7.md"
            | "docs/verification-0.3.8.md"
            | "docs/launcher-language-review.md"
    ) || (path.starts_with("src/") && path.ends_with(".rs"))
}

fn git(root: &Path, arguments: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Git source verification failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

fn source_snapshot(root: &Path, commit: &str) -> Result<BTreeMap<String, Vec<u8>>> {
    if commit.len() != 40 || !commit.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("SOURCE_COMMIT must be an exact 40-character Git commit".into());
    }
    let head = String::from_utf8(git(root, &["rev-parse", "HEAD"])?).map_err(|e| e.to_string())?;
    if head.trim() != commit.to_ascii_lowercase() {
        return Err("Freeze the reviewed source on the existing main HEAD before packaging".into());
    }
    if String::from_utf8(git(root, &["branch", "--show-current"])?)
        .map_err(|e| e.to_string())?
        .trim()
        != "main"
    {
        return Err("Launcher preparation requires the existing main checkout".into());
    }
    for changed in [
        git(root, &["diff", "--name-only", "-z", commit, "--"])?,
        git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?,
    ] {
        for path in changed.split(|c| *c == 0).filter(|p| !p.is_empty()) {
            let path = std::str::from_utf8(path).map_err(|e| e.to_string())?;
            if source_path(path) {
                return Err(format!("Public release source is not frozen: {path}"));
            }
        }
    }
    let mut source = BTreeMap::new();
    let paths = git(root, &["ls-tree", "-r", "--name-only", "-z", commit])?;
    for path in paths.split(|c| *c == 0).filter(|p| !p.is_empty()) {
        let path = std::str::from_utf8(path).map_err(|e| e.to_string())?;
        if source_path(path) {
            source.insert(
                path.into(),
                git(root, &["show", &format!("{commit}:{path}")])?,
            );
        }
    }
    for required in [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "build.rs",
        "src/main.rs",
        "src/lib.rs",
        "src/bin/launcher_cli.rs",
        "src/release.rs",
        "src/launcher_release.rs",
        "assets/Inter-Regular.ttf",
        "assets/Inter-Bold.ttf",
        "assets/paladinscat.png",
        "examples/verify_icon.rs",
        "examples/native_loader_fixture.rs",
    ]
    .into_iter()
    .chain(NOTICES.map(|(path, _)| path))
    {
        if !source.contains_key(required) {
            return Err(format!("Corresponding source is missing {required}"));
        }
    }
    let manifest = std::str::from_utf8(&source["Cargo.toml"]).map_err(|e| e.to_string())?;
    let mut package_section = false;
    let mut version = None;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            package_section = line == "[package]";
        }
        if package_section && let Some(value) = quoted_field(line, "version") {
            version = Some(value);
        }
    }
    if version.as_deref() != Some(env!("CARGO_PKG_VERSION")) {
        return Err("Source package version differs from this packaging tool".into());
    }
    Ok(source)
}

fn quoted_field(line: &str, name: &str) -> Option<String> {
    let (key, value) = line.split_once('=')?;
    let value = value.trim();
    (key.trim() == name && value.starts_with('"') && value.ends_with('"'))
        .then(|| value[1..value.len() - 1].to_owned())
}

fn locked_crates(lock: &[u8]) -> Result<Vec<LockedCrate>> {
    let text = std::str::from_utf8(lock).map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for block in text.split("[[package]]").skip(1) {
        let mut fields = BTreeMap::new();
        for line in block.lines() {
            for key in ["name", "version", "source", "checksum"] {
                if let Some(value) = quoted_field(line.trim(), key) {
                    fields.insert(key, value);
                }
            }
        }
        let Some(source) = fields.get("source") else {
            continue;
        };
        if source != "registry+https://github.com/rust-lang/crates.io-index" {
            return Err(
                "Non-crates.io source requires an explicit corresponding-source implementation"
                    .into(),
            );
        }
        let name = fields
            .get("name")
            .ok_or("Locked crate name absent")?
            .clone();
        let version = fields
            .get("version")
            .ok_or("Locked crate version absent")?
            .clone();
        let checksum = fields
            .get("checksum")
            .ok_or("Locked crate checksum absent")?
            .clone();
        if !safe_member(&name)
            || name.contains('/')
            || !safe_member(&version)
            || version.contains('/')
            || checksum.len() != 64
            || !checksum.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err("Unsafe locked dependency identity or checksum".into());
        }
        result.push(LockedCrate {
            name,
            version,
            checksum,
        });
    }
    if result.is_empty() {
        return Err("No locked dependency sources found".into());
    }
    Ok(result)
}

fn plain(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err("Source links are not accepted".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("Source reparse points are not accepted".into());
        }
    }
    Ok(())
}

fn files_under(root: &Path, directory: &Path, files: &mut BTreeSet<String>) -> Result<()> {
    plain(directory)?;
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        plain(&path)?;
        if path.is_dir() {
            files_under(root, &path, files)?;
        } else if path.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if !safe_member(&relative) {
                return Err("Unsafe vendored source path".into());
            }
            files.insert(relative);
        } else {
            return Err("Unsupported vendored source file".into());
        }
    }
    Ok(())
}

fn validate_vendor(directory: &Path, package: LockedCrate) -> Result<VendorCrate> {
    plain(directory)?;
    plain(&directory.join(".cargo-checksum.json"))?;
    let checksums: serde_json::Value = serde_json::from_slice(
        &fs::read(directory.join(".cargo-checksum.json"))
            .map_err(|e| format!("Missing vendored checksum for {}: {e}", package.name))?,
    )
    .map_err(|e| e.to_string())?;
    if checksums["package"].as_str() != Some(&package.checksum) {
        return Err(format!(
            "Vendored package checksum differs from Cargo.lock: {}",
            package.name
        ));
    }
    let mut files = BTreeMap::new();
    for (path, hash) in checksums["files"]
        .as_object()
        .ok_or("Vendored source file checksums absent")?
    {
        let hash = hash.as_str().ok_or("Invalid vendored source hash")?;
        if !safe_member(path) || hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("Unsafe vendored member or hash".into());
        }
        let file = directory.join(path);
        plain(&file)?;
        if artifact(&file)?.sha256 != hash {
            return Err(format!("Vendored source changed: {path}"));
        }
        files.insert(path.clone(), hash.into());
    }
    if files.is_empty() || !files.contains_key("Cargo.toml") {
        return Err("Vendored crate has no complete source inventory".into());
    }
    let mut actual = BTreeSet::new();
    files_under(directory, directory, &mut actual)?;
    actual.remove(".cargo-checksum.json");
    if actual != files.keys().cloned().collect() {
        return Err("Vendored source membership differs from its checksum inventory".into());
    }
    Ok(VendorCrate {
        package,
        directory: directory.into(),
        files,
    })
}

fn vendor_sources(bin: &Path, source: &BTreeMap<String, Vec<u8>>) -> Result<Vec<VendorCrate>> {
    let root = bin.join("vendor");
    if !root.is_dir() {
        return Err("Complete dependency sources missing; run cargo vendor --locked --offline --versioned-dirs RELEASE_BIN_DIR/vendor".into());
    }
    plain(&root)?;
    let mut result = Vec::new();
    for package in locked_crates(&source["Cargo.lock"])? {
        let versioned = root.join(format!("{}-{}", package.name, package.version));
        let directory = if versioned.is_dir() {
            versioned
        } else {
            root.join(&package.name)
        };
        result.push(validate_vendor(&directory, package)?);
    }
    Ok(result)
}

fn binary_version(bytes: &[u8], version: &str) -> Result<()> {
    paladinscat_launcher::self_update::validate_executable(bytes, version)
}

fn new_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut output = new_file(path)?;
    output
        .write_all(bytes)
        .and_then(|()| output.sync_all())
        .map_err(|e| e.to_string())
}

fn write_executables(
    output: &Path,
    binary_data: &BTreeMap<&str, Vec<u8>>,
) -> Result<Vec<Artifact>> {
    if binary_data.keys().copied().collect::<BTreeSet<_>>() != BINARIES.into_iter().collect() {
        return Err("Release requires exactly the GUI and CLI EXEs".into());
    }
    let mut artifacts = Vec::new();
    for (name, bytes) in binary_data {
        binary_version(bytes, env!("CARGO_PKG_VERSION"))?;
        write_new(&output.join(name), bytes)?;
        artifacts.push(artifact(&output.join(name))?);
    }
    Ok(artifacts)
}

fn add_bytes<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    path: &str,
    bytes: &[u8],
    inventory: &mut BTreeMap<String, String>,
) -> Result<()> {
    if !safe_member(path) || inventory.contains_key(path) {
        return Err("Unsafe or repeated archive member".into());
    }
    zip.start_file(
        path,
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default())
            .unix_permissions(0o644),
    )
    .map_err(|e| e.to_string())?;
    zip.write_all(bytes).map_err(|e| e.to_string())?;
    inventory.insert(path.into(), sha(bytes));
    Ok(())
}

fn finish_archive(
    zip: ZipWriter<File>,
    path: &Path,
    expected: &BTreeMap<String, String>,
) -> Result<Artifact> {
    zip.finish()
        .map_err(|e| e.to_string())?
        .sync_all()
        .map_err(|e| e.to_string())?;
    verify_archive(path, expected)?;
    artifact(path)
}

fn verify_archive(path: &Path, expected: &BTreeMap<String, String>) -> Result<()> {
    verify_zip(File::open(path).map_err(|e| e.to_string())?, expected)
}

fn verify_zip<R: Read + Seek>(reader: R, expected: &BTreeMap<String, String>) -> Result<()> {
    let mut archive = ZipArchive::new(reader).map_err(|e| e.to_string())?;
    if archive.len() != expected.len() {
        return Err("Archive membership count differs from its allowlist".into());
    }
    let mut names = BTreeSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let name = entry.name().to_owned();
        if !safe_member(&name)
            || !names.insert(name.clone())
            || entry.is_dir()
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
            || !expected.contains_key(&name)
        {
            return Err("Archive contains an unexpected, duplicate or unsafe member".into());
        }
        let mut digest = Sha256::new();
        let mut buffer = [0; 131_072];
        loop {
            let count = entry.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        if format!("{:x}", digest.finalize()) != expected[&name] {
            return Err("Archive member hash differs from its source".into());
        }
    }
    Ok(())
}

fn prepare_inner(
    root: &Path,
    bin: &Path,
    output: &Path,
    commit: &str,
    catalog: &[u8],
) -> Result<()> {
    engine::parse_catalog(catalog)?;
    let source = source_snapshot(root, commit)?;
    let vendor = vendor_sources(bin, &source)?;
    let mut binary_data = BTreeMap::new();
    for name in BINARIES {
        let path = bin.join(name);
        plain(&path)?;
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        binary_version(&bytes, env!("CARGO_PKG_VERSION"))?;
        binary_data.insert(name, bytes);
    }
    fs::create_dir(output).map_err(|e| format!("Release directory must be new: {e}"))?;
    let prepared = (|| -> Result<()> {
        let version = env!("CARGO_PKG_VERSION");
        let source_name = format!("PaladinsCat-Launcher-{version}-corresponding-source.zip");
        let mut binaries = write_executables(output, &binary_data)?;
        // Notices are separate release assets. Their basenames are unique; the
        // corresponding source archive retains the original directory layout.
        for (from, to) in NOTICES {
            let name = Path::new(to).file_name().ok_or("Notice filename absent")?;
            write_new(&output.join(name), &source[from])?;
        }
        let mut archive = ZipWriter::new(new_file(&output.join(&source_name))?);
        let mut source_files = BTreeMap::new();
        for (path, bytes) in &source {
            add_bytes(&mut archive, path, bytes, &mut source_files)?;
        }
        let config = b"[source.crates-io]\nreplace-with = \"vendored-sources\"\n\n[source.vendored-sources]\ndirectory = \"vendor\"\n";
        add_bytes(
            &mut archive,
            ".cargo/config.toml",
            config,
            &mut source_files,
        )?;
        let building = format!(
            "PaladinsCat Launcher corresponding source\nSource commit: {commit}\n\nOn Windows x64, use the Rust toolchain pinned in rust-toolchain.toml and the Windows SDK.\nAll Cargo.lock registry sources are included under vendor with verified Cargo checksums.\n\ncargo build --locked --offline --release --bin paladinscat-launcher --bin paladinscat-launcher-cli --bin paladinscat-release\ncargo test --locked --offline --all-targets\n\nNo Tempest installation, game files or private WatchCat checkout is required to build the launcher.\n"
        );
        add_bytes(
            &mut archive,
            "BUILDING.txt",
            building.as_bytes(),
            &mut source_files,
        )?;
        let mut dependency_index = Vec::new();
        for dependency in &vendor {
            let prefix = format!(
                "vendor/{}-{}",
                dependency.package.name, dependency.package.version
            );
            for (path, expected) in &dependency.files {
                let bytes = fs::read(dependency.directory.join(path)).map_err(|e| e.to_string())?;
                if sha(&bytes) != *expected {
                    return Err("Dependency source changed during archive preparation".into());
                }
                add_bytes(
                    &mut archive,
                    &format!("{prefix}/{path}"),
                    &bytes,
                    &mut source_files,
                )?;
            }
            let checksum = serde_json::to_vec(&serde_json::json!({"package":dependency.package.checksum,"files":dependency.files})).map_err(|e| e.to_string())?;
            add_bytes(
                &mut archive,
                &format!("{prefix}/.cargo-checksum.json"),
                &checksum,
                &mut source_files,
            )?;
            dependency_index.push(serde_json::json!({"name":dependency.package.name,"version":dependency.package.version,"registry_package_sha256":dependency.package.checksum,"files":dependency.files.len()}));
        }
        add_bytes(
            &mut archive,
            "dependency-sources.json",
            &serde_json::to_vec_pretty(&dependency_index).map_err(|e| e.to_string())?,
            &mut source_files,
        )?;
        let corresponding = finish_archive(archive, &output.join(&source_name), &source_files)?;
        write_new(&output.join("catalog.json"), catalog)?;
        let catalog_artifact = artifact(&output.join("catalog.json"))?;
        let manifest = serde_json::json!({
            "schema_version":2,"product":"PaladinsCat Launcher","version":version,"platform":"windows-x64",
            "source_commit":commit,"source_url":format!("https://github.com/NabiCook/PaladinsCat-Launcher/tree/{commit}"),
            "executables":binaries,"corresponding_source":corresponding,"dependency_sources":vendor.len(),
            "self_update":{"repository":paladinscat_launcher::sources::LAUNCHER_REPOSITORY,"tag":format!("v{version}"),"asset":paladinscat_launcher::self_update::EXE,"required_github_digest":"sha256"},
            "dependencies_complete":true,"catalog":catalog_artifact,"catalog_entries":serde_json::from_slice::<serde_json::Value>(catalog).map_err(|e|e.to_string())?,
            "source_members":source_files,
            "game_files_changed":false,"gameplay_acceptance_verified":false,"visual_acceptance_verified":false,"localization_complete":false
        });
        write_new(
            &output.join("release-manifest.json"),
            &serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
        )?;
        let manifest_artifact = artifact(&output.join("release-manifest.json"))?;
        binaries.extend([corresponding, catalog_artifact, manifest_artifact]);
        for (_, to) in NOTICES {
            binaries.push(artifact(
                &output.join(Path::new(to).file_name().ok_or("Notice filename absent")?),
            )?);
        }
        let checksums = binaries
            .iter()
            .map(|item| format!("{}  {}\n", item.sha256, item.file))
            .collect::<String>();
        write_new(&output.join("SHA256SUMS.txt"), checksums.as_bytes())?;
        Ok(())
    })();
    if let Err(error) = &prepared {
        let evidence = serde_json::to_vec_pretty(
            &serde_json::json!({"complete":false,"source_commit":commit,"error":error}),
        )
        .map_err(|e| e.to_string())?;
        let _ = write_new(&output.join("preparation-error.json"), &evidence);
    }
    prepared
}

pub fn prepare(bin: &Path, output: &Path, commit: &str, catalog: &Path) -> Result<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let expected = root.join("dist").join(env!("CARGO_PKG_VERSION"));
    let parent = output
        .parent()
        .ok_or("Release output parent absent")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if parent.join(output.file_name().ok_or("Release output name absent")?) != expected
        || output.exists()
    {
        return Err("OUTPUT_DIR must be the new dist/<current-version> directory; existing staging is preserved".into());
    }
    plain(&parent)?;
    let bin = bin.canonicalize().map_err(|e| e.to_string())?;
    plain(&bin)?;
    let catalog = fs::read(catalog).map_err(|e| e.to_string())?;
    prepare_inner(&root, &bin, &expected, commit, &catalog)?;
    Ok(expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_release_assets_are_direct_pe_files_and_never_a_launcher_zip() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/release-contract-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&root).unwrap();
        let mut bytes = vec![0; 128];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&[0x64, 0x86]);
        bytes[88..90].copy_from_slice(&[0x0b, 0x02]);
        bytes[96..100].copy_from_slice(&[0xbd, 0x04, 0xef, 0xfe]);
        let version = env!("CARGO_PKG_VERSION")
            .split('.')
            .map(|p| p.parse::<u32>().unwrap())
            .collect::<Vec<_>>();
        bytes[104..108].copy_from_slice(&((version[0] << 16) | version[1]).to_le_bytes());
        bytes[108..112].copy_from_slice(&(version[2] << 16).to_le_bytes());
        let binaries = BINARIES
            .into_iter()
            .map(|name| (name, bytes.clone()))
            .collect();
        let artifacts = write_executables(&root, &binaries).unwrap();
        assert_eq!(artifacts.len(), 2);
        for artifact in artifacts {
            assert!(artifact.file.ends_with(".exe"));
            assert_eq!(fs::read(root.join(artifact.file)).unwrap(), bytes);
            assert_eq!(artifact.sha256, sha(&bytes));
        }
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        assert!(write_executables(&root, &binaries).is_err());
    }

    #[test]
    fn public_source_allowlist_excludes_runtime_private_helpers_and_traversal() {
        for path in [
            "src/main.rs",
            "src/bin/launcher_cli.rs",
            "examples/native_loader_fixture.rs",
            "assets/paladinscat.png",
        ] {
            assert!(source_path(path));
        }
        for path in [
            ".env",
            "local/runtime/secrets/token.txt",
            "target/release/a.exe",
            "examples/interactive_launch_test.rs",
            "examples/package_operation.rs",
            "scripts/run.ps1",
            "docs/private-appendix.md",
            "../src/main.rs",
            "C:/src/main.rs",
            "src/../private.rs",
            "src\\main.rs",
            "/src/main.rs",
        ] {
            assert!(!source_path(path), "{path}");
        }
    }

    #[test]
    fn lock_parser_requires_exact_sources_and_checksums() {
        let valid = format!(
            "[[package]]\nname = \"sample\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{}\"\n",
            "a".repeat(64)
        );
        assert_eq!(locked_crates(valid.as_bytes()).unwrap().len(), 1);
        assert!(locked_crates(valid.replace("sample", "../sample").as_bytes()).is_err());
        assert!(
            locked_crates(
                valid
                    .replace(
                        "registry+https://github.com/rust-lang/crates.io-index",
                        "git+https://example.invalid"
                    )
                    .as_bytes()
            )
            .is_err()
        );
        assert!(locked_crates(valid.replace(&"a".repeat(64), "wrong").as_bytes()).is_err());
    }

    #[test]
    fn old_or_non_x64_executables_are_not_packaged() {
        let mut bytes = vec![0; 128];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&[0x64, 0x86]);
        bytes[88..90].copy_from_slice(&[0x0b, 0x02]);
        bytes[96..100].copy_from_slice(&[0xbd, 0x04, 0xef, 0xfe]);
        bytes[104..108].copy_from_slice(&3u32.to_le_bytes());
        bytes[108..112].copy_from_slice(&(7u32 << 16).to_le_bytes());
        assert!(binary_version(&bytes, "0.3.7").is_ok());
        assert!(binary_version(&bytes, "0.3.6").is_err());
        bytes[68] = 0x4c;
        assert!(binary_version(&bytes, "0.3.7").is_err());
    }

    #[test]
    fn unsafe_commit_is_refused_before_git_or_output_mutation() {
        for commit in [
            "HEAD",
            "../replace",
            "abc",
            "--all",
            "fffffffffffffffffffffffffffffffffffffffz",
        ] {
            assert!(
                source_snapshot(Path::new("does-not-exist"), commit)
                    .unwrap_err()
                    .contains("40-character")
            );
        }
    }

    #[test]
    fn archive_verification_rejects_unexpected_names_and_changed_content() {
        let mut zip = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let mut expected = BTreeMap::new();
        add_bytes(&mut zip, "LICENSE", b"license", &mut expected).unwrap();
        assert!(add_bytes(&mut zip, "LICENSE", b"replace", &mut expected).is_err());
        assert!(add_bytes(&mut zip, "../private", b"private", &mut expected).is_err());
        let bytes = zip.finish().unwrap().into_inner();
        assert!(verify_zip(std::io::Cursor::new(&bytes), &expected).is_ok());
        assert!(
            verify_zip(
                std::io::Cursor::new(&bytes),
                &BTreeMap::from([("NOTICE.md".into(), sha(b"license"))])
            )
            .is_err()
        );
        assert!(
            verify_zip(
                std::io::Cursor::new(&bytes),
                &BTreeMap::from([("LICENSE".into(), sha(b"changed"))])
            )
            .is_err()
        );
    }
}
