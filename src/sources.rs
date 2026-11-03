//! Public package sources shared by download code, release tooling and feature tabs.

pub const LAUNCHER_REPOSITORY: &str = "NabiCook/PaladinsCat-Launcher";
pub const LAUNCHER_RELEASE_API: &str =
    "https://api.github.com/repos/NabiCook/PaladinsCat-Launcher/releases/latest";

pub const LOCALE_REPOSITORY: &str = "NabiCook/PaladinsCat-locales";
pub const PERFORMANCE_REPOSITORY: &str = "NabiCook/PaladinsCat-mods";
pub const PERFORMANCE_RELEASE_API: &str =
    "https://api.github.com/repos/NabiCook/PaladinsCat-mods/releases/latest";
pub const PERFORMANCE_RELEASE_URL: &str =
    "https://github.com/NabiCook/PaladinsCat-mods/releases/latest";

pub fn canonical_repository(repository: &str) -> &str {
    match repository.trim() {
        "PaladinsCat/PaladinsCat-Launcher" => LAUNCHER_REPOSITORY,
        "PaladinsCat/PaladinsCat-locales" => LOCALE_REPOSITORY,
        "PaladinsCat/PaladinsCat-mods" => PERFORMANCE_REPOSITORY,
        other => other,
    }
}

pub fn canonical_source_url(url: &str) -> String {
    let url = url.trim();
    for (old, new) in [
        (
            "https://raw.githubusercontent.com/PaladinsCat/PaladinsCat-locales/",
            "https://raw.githubusercontent.com/NabiCook/PaladinsCat-locales/",
        ),
        (
            "https://github.com/PaladinsCat/PaladinsCat-locales/",
            "https://github.com/NabiCook/PaladinsCat-locales/",
        ),
    ] {
        if let Some(path) = url.strip_prefix(old) {
            return format!("{new}{path}");
        }
    }
    url.into()
}

pub fn repository_url(repository: &str) -> String {
    format!("https://github.com/{}", canonical_repository(repository))
}

pub fn catalog_url(repository: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/{}/main/game-client/catalog.json",
        canonical_repository(repository)
    )
}

pub fn performance_asset_url(version: &str, name: &str) -> String {
    format!("https://github.com/{PERFORMANCE_REPOSITORY}/releases/download/v{version}/{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_urls_keep_their_own_repository_and_release_contracts() {
        assert!(catalog_url(LOCALE_REPOSITORY).contains("PaladinsCat-locales/main/game-client/"));
        assert!(repository_url(PERFORMANCE_REPOSITORY).ends_with("/PaladinsCat-mods"));
        assert_eq!(
            performance_asset_url("0.1.2", "manifest.json"),
            "https://github.com/NabiCook/PaladinsCat-mods/releases/download/v0.1.2/manifest.json"
        );
        assert!(!PERFORMANCE_RELEASE_API.contains("PaladinsCat-locales"));
    }

    #[test]
    fn saved_organization_sources_follow_the_same_repositories_to_the_owner() {
        assert_eq!(
            catalog_url("PaladinsCat/PaladinsCat-locales"),
            catalog_url(LOCALE_REPOSITORY)
        );
        assert_eq!(
            canonical_source_url(
                "https://raw.githubusercontent.com/PaladinsCat/PaladinsCat-locales/main/game-client/catalog.json"
            ),
            catalog_url(LOCALE_REPOSITORY)
        );
        assert_eq!(
            canonical_source_url(
                "https://github.com/PaladinsCat/PaladinsCat-locales/releases/download/preview/catalog.json"
            ),
            "https://github.com/NabiCook/PaladinsCat-locales/releases/download/preview/catalog.json"
        );
    }

    #[test]
    fn custom_sources_and_similar_repository_names_retain_their_owner() {
        for repository in [
            "contributor/PaladinsCat-locales",
            "PaladinsCat/PaladinsCat-locales-other",
        ] {
            assert_eq!(canonical_repository(repository), repository);
        }
        let custom = "https://raw.githubusercontent.com/PaladinsCat/PaladinsCat-locales-other/main/catalog.json";
        assert_eq!(canonical_source_url(custom), custom);
    }
}
