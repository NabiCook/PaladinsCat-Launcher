//! Immutable preview preparation; no publication or game installation.
use paladinscat_launcher::{engine, ime, sources};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

fn sha(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 131072];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
pub fn assemble(
    bundle: &Path,
    ime_manifest: &Path,
    repository: &Path,
    tag: &str,
    locale_commit: &str,
    tooling_commit: &str,
) -> Result<PathBuf, String> {
    if tag.is_empty()
        || tag.len() > 100
        || !tag
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
        || [locale_commit, tooling_commit]
            .iter()
            .any(|v| v.len() != 40 || !v.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return Err("Use an immutable safe tag and exact source commits".into());
    }
    let cache = engine::data_dir();
    let cached = engine::import(bundle, &cache, &|p| println!("{p}"))?;
    let ime_cached = ime::import(ime_manifest, &cache)?;
    let output = repository
        .join("game-client/.release-staging/releases")
        .join(tag);
    if output.exists() {
        return Err(
            "Release preparation directory already exists; never replace immutable assets".into(),
        );
    }
    fs::create_dir_all(&output).map_err(|e| e.to_string())?;
    let base = format!(
        "{}/releases/download/{tag}",
        sources::repository_url(sources::LOCALE_REPOSITORY)
    );
    let mut package = cached.package;
    package.source_commit = locale_commit.into();
    package.game_build = ime_cached.package.game_build.clone();
    package.game_fingerprint = Some(ime_cached.package.game_fingerprint.clone());
    package.min_tool_version = Some([0, 2, 0]);
    // Locale identity and public copy belong to the supplied reviewed bundle.
    write_locale_payloads(&mut package, &cached.directory, &output, &base)?;
    let mut ime_package = ime_cached.package;
    ime_package.draft = false;
    ime_package.source_commit = tooling_commit.into();
    ime_package.notes = "PREVIEW · Korean IME compatibility for pinned Win64 8.1. Esc, rapid-I and Korean typing passed on frozen v4-r1; this portable build and live disable still require a fresh client trial.".into();
    ime_package.download = Some(engine::Download {
        url: format!("{base}/{}", ime::DLL),
        sha256: ime_package.dll_sha256.clone(),
        bytes: ime_package.dll_bytes,
        encoding: "raw".into(),
    });
    fs::copy(ime_cached.directory.join(ime::DLL), output.join(ime::DLL))
        .map_err(|e| e.to_string())?;
    engine::save_settings(&output.join("ime-package.json"), &ime_package)?;
    // Actual PE/ABI, identity and payload verification on the finalized public manifest.
    ime::import(&output.join("ime-package.json"), &cache)?;
    let catalog = serde_json::json!({"schema_version":1,
        "packages":[locale_entry(&package, &output, &base)?],
        "ime_packages":[ime::Release {package_id:ime_package.package_id,version:ime_package.version,game_build:ime_package.game_build,manifest_url:format!("{base}/ime-package.json"),manifest_sha256:sha(&output.join("ime-package.json"))?,notes:ime_package.notes}]
    });
    engine::parse_catalog(&serde_json::to_vec(&catalog).map_err(|e| e.to_string())?)?;
    engine::save_settings(&output.join("catalog.json"), &catalog)?;
    engine::save_settings(
        &output.join("provenance.json"),
        &serde_json::json!({"schema_version":1,"channel":"preview","locale_source_commit":locale_commit,"ime_source_commit":tooling_commit,"packaging_binary_sha256":sha(&std::env::current_exe().map_err(|e|e.to_string())?)?,"native_bundle_id":package.bundle_id,"localization_complete":false,"portable_ime_live_verified":false,"game_files_changed":false}),
    )?;
    Ok(output)
}

fn write_locale_payloads(
    package: &mut engine::Package,
    cache: &Path,
    output: &Path,
    base: &str,
) -> Result<(), String> {
    for payload in &mut package.files {
        let name = Path::new(&payload.source)
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or("Invalid asset filename")?;
        let asset = format!("{name}.gz");
        let destination = output.join(&asset);
        if destination.exists() {
            return Err("Duplicate release asset filename".into());
        }
        let mut input = File::open(cache.join(&payload.source)).map_err(|e| e.to_string())?;
        let mut encoder = flate2::write::GzEncoder::new(
            File::create(&destination).map_err(|e| e.to_string())?,
            flate2::Compression::default(),
        );
        let mut digest = Sha256::new();
        let mut bytes = 0u64;
        let mut buffer = [0; 131072];
        loop {
            let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            digest.update(&buffer[..n]);
            encoder.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
            bytes += n as u64;
        }
        encoder
            .finish()
            .map_err(|e| e.to_string())?
            .sync_all()
            .map_err(|e| e.to_string())?;
        if format!("{:x}", digest.finalize()) != payload.artifact_sha256 {
            return Err("Payload changed during release assembly".into());
        }
        payload.artifact_bytes = Some(bytes);
        payload.download = Some(engine::Download {
            url: format!("{base}/{asset}"),
            sha256: sha(&destination)?,
            bytes: fs::metadata(&destination).map_err(|e| e.to_string())?.len(),
            encoding: "gzip".into(),
        });
        println!(
            "Prepared {asset}: {} bytes",
            payload.download.as_ref().unwrap().bytes
        );
    }
    engine::save_settings(&output.join("locale-package.json"), package)
}

fn locale_entry(
    package: &engine::Package,
    output: &Path,
    base: &str,
) -> Result<engine::Release, String> {
    Ok(engine::Release {
        bundle_id: package.bundle_id.clone(),
        locale: package.locale.clone(),
        game_build: package.game_build.clone(),
        manifest_url: format!("{base}/locale-package.json"),
        manifest_sha256: sha(&output.join("locale-package.json"))?,
        notes: package.notes.clone(),
    })
}

fn validate_locale_request(tag: &str, commit: &str, notes: &str) -> Result<(), String> {
    if tag.is_empty()
        || tag.len() > 100
        || !tag
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
        || commit.len() != 40
        || !commit.bytes().all(|c| c.is_ascii_hexdigit())
        || notes.trim().is_empty()
        || notes.chars().count() > 180
    {
        return Err("Use a safe immutable tag, exact locale commit and short preview copy".into());
    }
    Ok(())
}

fn update_locale_catalog(
    original: &[u8],
    release: engine::Release,
) -> Result<serde_json::Value, String> {
    engine::parse_catalog(original)?;
    let mut catalog: serde_json::Value =
        serde_json::from_slice(original).map_err(|e| e.to_string())?;
    catalog["packages"] = serde_json::json!([release]);
    engine::parse_catalog(&serde_json::to_vec(&catalog).map_err(|e| e.to_string())?)?;
    Ok(catalog)
}

/// Prepare localization alone; retain the repository's independent IME references.
pub fn assemble_locale(
    bundle: &Path,
    repository: &Path,
    tag: &str,
    locale_commit: &str,
    notes: &str,
) -> Result<PathBuf, String> {
    validate_locale_request(tag, locale_commit, notes)?;
    let original_catalog =
        fs::read(repository.join("game-client/catalog.json")).map_err(|e| e.to_string())?;
    engine::parse_catalog(&original_catalog)?;
    let cached = engine::import(bundle, &engine::data_dir(), &|p| println!("{p}"))?;
    let mut package = cached.package;
    if package.game_fingerprint.is_none() {
        return Err("A remote locale needs a pinned game fingerprint".into());
    }
    package.source_commit = locale_commit.into();
    package.notes = notes.into();
    let output = repository
        .join("game-client/.release-staging/releases")
        .join(tag);
    if output.exists() {
        return Err("Release preparation already exists; never replace immutable assets".into());
    }
    fs::create_dir_all(&output).map_err(|e| e.to_string())?;
    let base = format!(
        "{}/releases/download/{tag}",
        sources::repository_url(sources::LOCALE_REPOSITORY)
    );
    write_locale_payloads(&mut package, &cached.directory, &output, &base)?;
    let catalog =
        update_locale_catalog(&original_catalog, locale_entry(&package, &output, &base)?)?;
    engine::save_settings(&output.join("catalog.json"), &catalog)?;
    engine::save_settings(
        &output.join("provenance.json"),
        &serde_json::json!({"schema_version":1,"channel":"preview","locale_source_commit":locale_commit,"packaging_binary_sha256":sha(&std::env::current_exe().map_err(|e|e.to_string())?)?,"native_bundle_id":package.bundle_id,"localization_complete":false,"visual_acceptance_verified":false,"ime_entry_changed":false,"game_files_changed":false,"original_catalog_sha256":format!("{:x}",Sha256::digest(&original_catalog)),"packaging_release_source_sha256":format!("{:x}",Sha256::digest(include_bytes!("release.rs"))),"packaging_assets_source_sha256":format!("{:x}",Sha256::digest(include_bytes!("release_assets.rs")))}),
    )?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_publication_preserves_independent_ime() {
        let original = serde_json::json!({"schema_version":1,"packages":[],"ime_packages":[{"package_id":"ime-existing","version":"0.1.0","game_build":"8.1","manifest_url":"https://github.com/NabiCook/PaladinsCat-locales/releases/download/old/ime-package.json","manifest_sha256":"a".repeat(64),"notes":"unchanged"}]});
        let release = engine::Release {bundle_id:"kor-ui-v174".into(),locale:"KOR".into(),game_build:"8.1".into(),manifest_url:"https://github.com/NabiCook/PaladinsCat-locales/releases/download/new/locale-package.json".into(),manifest_sha256:"b".repeat(64),notes:"PREVIEW · Korean v174.".into()};
        let updated =
            update_locale_catalog(&serde_json::to_vec(&original).unwrap(), release).unwrap();
        assert_eq!(updated["ime_packages"], original["ime_packages"]);
        assert_eq!(updated["packages"][0]["bundle_id"], "kor-ui-v174");
        assert!(
            update_locale_catalog(
                b"{}",
                engine::Release {
                    bundle_id: String::new(),
                    locale: String::new(),
                    game_build: String::new(),
                    manifest_url: String::new(),
                    manifest_sha256: String::new(),
                    notes: String::new()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn locale_request_rejects_unsafe_tags_and_missing_provenance() {
        let commit = "a".repeat(40);
        assert!(validate_locale_request("preview-v174", &commit, "PREVIEW · Korean v174.").is_ok());
        assert!(validate_locale_request("../replace", &commit, "preview").is_err());
        assert!(validate_locale_request("preview", "", "preview").is_err());
        assert!(validate_locale_request("preview", &commit, &"x".repeat(181)).is_err());
    }
}
