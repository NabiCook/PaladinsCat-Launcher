//! Verify both input variants against a prepared package without cache writes,
//! game dispatch, DLL execution or changes to an existing process.
use paladinscat_launcher::{launch, performance};
use serde_json::json;
use std::{error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: verify_performance_launch GAME_ROOT PREPARED_MANIFEST PROFILE".into());
    }
    let manifest = PathBuf::from(&args[1]);
    let cached = performance::Cached {
        package: serde_json::from_slice(&fs::read(&manifest)?)?,
        directory: manifest.parent().ok_or("manifest parent")?.canonicalize()?,
    };
    performance::verify(&cached)?;
    if cached.package.version == "0.1.1" {
        let cache = cached
            .directory
            .parent()
            .and_then(|p| p.parent())
            .ok_or("cache root")?;
        let restored = performance::import(&manifest, cache)?;
        if restored.package != cached.package || restored.directory != cached.directory {
            return Err("prepared package startup restore differs".into());
        }
    }
    let mut variants = Vec::new();
    for previous in [false, true] {
        let plan = launch::plan(&launch::Options {
            game_root: PathBuf::from(&args[0]),
            language: "INT".into(),
            homedir: args[2].to_str().ok_or("non-Unicode profile")?.into(),
            platform: launch::LaunchPlatform::Steam,
            performance: Some(cached.clone()),
            rollback_input: previous,
        })?;
        let selected = performance::dlls(&cached, previous)?;
        let expected = performance::PAYLOADS[if previous { 2 } else { 0 }];
        let queued_package = true;
        let world_selected = plan.dlls.iter().any(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("watchcat-world-scheduler.dll"))
        });
        if selected.len() != if queued_package { 2 } else { 1 }
            || selected[0].file_name().and_then(|name| name.to_str()) != Some(expected)
            || !plan.dlls.contains(&selected[0].canonicalize()?)
            || world_selected != queued_package
        {
            return Err("launch plan failed input-only/native-worker policy".into());
        }
        variants.push(json!({"previous_input":previous,"load_dlls":plan.dlls}));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "read_only":true,"game_launched":false,"cache_modified":false,
            "native_world_worker_queues":true,"variants":variants
        }))?
    );
    Ok(())
}
