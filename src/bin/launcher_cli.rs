//! Explicit CLI operations for package preparation and read-only launch checks.
use paladinscat_launcher::{
    engine, game_discovery, graphics_policy, launch, performance, self_update,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

struct LaunchRequest {
    options: launch::Options,
    performance_manifest: Option<PathBuf>,
    cache_root: PathBuf,
}

fn cache_directory_kind(directory: bool, link: bool, attributes: u32) -> engine::Result<()> {
    if link || attributes & 0x400 != 0 {
        return Err("Cache root links and reparse points are not supported".into());
    }
    if !directory {
        return Err("Cache root must be an existing directory".into());
    }
    Ok(())
}

fn plain_cache_ancestors(path: &Path) -> engine::Result<()> {
    for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        let metadata = fs::symlink_metadata(ancestor).map_err(|error| {
            format!("Cannot inspect cache root {}: {error}", ancestor.display())
        })?;
        #[cfg(windows)]
        let attributes = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes()
        };
        #[cfg(not(windows))]
        let attributes = 0;
        cache_directory_kind(
            metadata.is_dir(),
            metadata.file_type().is_symlink(),
            attributes,
        )?;
    }
    Ok(())
}

fn explicit_cache_root(value: &str) -> engine::Result<PathBuf> {
    if value.is_empty() || value.starts_with("--") {
        return Err("Missing cache-root option value".into());
    }
    let supplied = Path::new(value);
    let absolute = if supplied.is_absolute() {
        supplied.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(supplied)
    };
    // Inspect before resolving links, including links in parent components.
    // Downstream import/cleanup retains its independent ownership checks.
    plain_cache_ancestors(&absolute)?;
    let canonical = absolute.canonicalize().map_err(|error| error.to_string())?;
    plain_cache_ancestors(&canonical)?;
    Ok(canonical)
}

fn parse_launch(args: &[String], default_cache: PathBuf) -> engine::Result<LaunchRequest> {
    if args.len() < 2 || !matches!(args[0].as_str(), "--dry-run" | "--launch") {
        return Err("Cache root is supported only by --dry-run and --launch".into());
    }
    let mut options = launch::Options {
        game_root: PathBuf::from(&args[1]),
        language: launch::DEFAULT_LANGUAGE.into(),
        homedir: launch::DEFAULT_PROFILE.into(),
        platform: launch::LaunchPlatform::default(),
        performance: None,
        rollback_input: false,
    };
    let (mut performance_manifest, mut cache_root) = (None, None);
    let mut index = 2;
    while index < args.len() {
        let flag = &args[index];
        if flag == "--previous-input" {
            if options.rollback_input {
                return Err("Duplicate previous-input option".into());
            }
            options.rollback_input = true;
            index += 1;
            continue;
        }
        let value = args.get(index + 1).ok_or("Missing option value")?;
        match flag.as_str() {
            "--cache-root" => {
                if cache_root.is_some() {
                    return Err("Duplicate cache-root option".into());
                }
                cache_root = Some(value.clone());
            }
            "--performance" => performance_manifest = Some(PathBuf::from(value)),
            "--language" => options.language.clone_from(value),
            "--profile" => options.homedir.clone_from(value),
            "--login-method" => options.platform = launch::LaunchPlatform::parse(value)?,
            _ => return Err("Unknown launcher option".into()),
        }
        index += 2;
    }
    Ok(LaunchRequest {
        options,
        performance_manifest,
        cache_root: cache_root
            .as_deref()
            .map(explicit_cache_root)
            .transpose()?
            .unwrap_or(default_cache),
    })
}

fn run() -> engine::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.as_slice() == ["--check-launcher-update"] {
        println!(
            "{}",
            serde_json::to_string_pretty(&self_update::check()?).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if args.as_slice() == ["--detect-games"] {
        println!(
            "{}",
            serde_json::to_string_pretty(&game_discovery::discover()).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if args.as_slice() == ["--data-dir"] {
        println!("{}", engine::data_dir().display());
        return Ok(());
    }
    if args.as_slice() == ["--pin-rtx-5080"] {
        let path = engine::data_dir().join("graphics-policy.json");
        graphics_policy::write_rtx_5080_policy(&path)?;
        println!("RTX 5080 graphics policy saved at {}", path.display());
        return Ok(());
    }
    if args.len() == 2 && args[0] == "download-performance" {
        let cached =
            performance::download(&PathBuf::from(&args[1]), &|message| println!("{message}"))?;
        println!("{}", performance::manifest_path(&cached).display());
        return Ok(());
    }
    if args.len() == 2 && args[0] == "--restore" {
        let game = PathBuf::from(&args[1]);
        let status = engine::operate(
            engine::Action::Restore,
            &game,
            None,
            &engine::data_dir(),
            &|message| println!("{message}"),
        )?;
        println!("{status}");
        return Ok(());
    }
    if args.len() >= 2 && matches!(args[0].as_str(), "--dry-run" | "--launch") {
        let LaunchRequest {
            mut options,
            performance_manifest,
            cache_root,
        } = parse_launch(&args, engine::data_dir())?;
        if let Some(manifest) = performance_manifest {
            options.performance = Some(performance::import(&manifest, &cache_root)?);
        }
        if args[0] == "--dry-run" {
            let plan = launch::plan(&options)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "game_executable": plan.game_executable,
                    "game_root": plan.game_root,
                    "working_directory": plan.working_directory,
                    "args": plan.args,
                    "load_dlls": plan.dlls,
                    "game_launched": false,
                    "hooks_verified": false
                }))
                .map_err(|e| e.to_string())?
            );
        } else {
            let result = launch::start(&options, &cache_root.join("launch-logs"))?;
            println!("{}", result.status);
        }
        return Ok(());
    }
    Err("Usage: paladinscat-launcher-cli --check-launcher-update | --detect-games | --data-dir | --pin-rtx-5080 | download-performance CACHE | --restore GAME_ROOT | --dry-run|--launch GAME_ROOT [--performance MANIFEST] [--cache-root EXISTING_DIRECTORY] [--previous-input] [--language KOR|INT] [--profile NAME] [--login-method pc|steam|epic]".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn default_cache_and_existing_launch_options_are_preserved() {
        let default = PathBuf::from("unchanged-default-cache");
        let request = parse_launch(
            &arguments(&[
                "--dry-run",
                "game",
                "--language",
                "KOR",
                "--profile",
                "custom",
                "--login-method",
                "steam",
                "--previous-input",
            ]),
            default.clone(),
        )
        .unwrap();
        assert_eq!(request.cache_root, default);
        assert_eq!(request.options.language, "KOR");
        assert_eq!(request.options.homedir, "custom");
        assert_eq!(request.options.platform, launch::LaunchPlatform::Steam);
        assert!(request.options.rollback_input);
        assert!(request.performance_manifest.is_none());
    }

    #[test]
    fn explicit_cache_is_resolved_before_import_regardless_of_option_order() {
        let root = env!("CARGO_MANIFEST_DIR");
        let canonical = Path::new(root).canonicalize().unwrap();
        for mode in ["--dry-run", "--launch"] {
            for options in [
                vec![
                    mode,
                    "game",
                    "--performance",
                    "not-imported.json",
                    "--cache-root",
                    root,
                ],
                vec![
                    mode,
                    "game",
                    "--cache-root",
                    root,
                    "--performance",
                    "not-imported.json",
                ],
            ] {
                let request = parse_launch(&arguments(&options), "unused".into()).unwrap();
                assert_eq!(request.cache_root, canonical);
                assert_eq!(
                    request.performance_manifest,
                    Some("not-imported.json".into())
                );
                assert!(request.options.performance.is_none());
            }
        }
    }

    #[test]
    fn cache_option_rejects_duplicate_missing_and_other_operations() {
        for args in [
            vec!["--launch", "game", "--cache-root"],
            vec!["--launch", "game", "--cache-root", ""],
            vec!["--launch", "game", "--cache-root", "--previous-input"],
            vec!["--launch", "game", "--cache-root", ".", "--cache-root", "."],
            vec!["--restore", "game", "--cache-root", "."],
        ] {
            assert!(parse_launch(&arguments(&args), "unused".into()).is_err());
        }
    }

    #[test]
    fn explicit_cache_requires_existing_plain_directory_and_canonicalizes_relative_paths() {
        assert_eq!(
            explicit_cache_root(".").unwrap(),
            std::env::current_dir().unwrap().canonicalize().unwrap()
        );
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(explicit_cache_root(root.join("Cargo.toml").to_str().unwrap()).is_err());
        let absent = root.join(format!("absent-cli-cache-{}", std::process::id()));
        assert!(!absent.exists());
        assert!(explicit_cache_root(absent.to_str().unwrap()).is_err());
        assert!(!absent.exists());
    }

    #[test]
    fn directory_metadata_gate_rejects_links_and_all_reparse_kinds() {
        assert!(cache_directory_kind(true, false, 0).is_ok());
        assert!(cache_directory_kind(false, false, 0).is_err());
        assert!(cache_directory_kind(true, true, 0).is_err());
        assert!(cache_directory_kind(true, false, 0x410).is_err());
        assert!(cache_directory_kind(false, false, 0x400).is_err());
    }
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
