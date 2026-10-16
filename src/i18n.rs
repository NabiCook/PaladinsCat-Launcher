//! Launcher interface strings. Korean is an initial translation for owner review.
//!
//! Interface language is independent of the game/package locale. Keep the
//! English source text as application state; translate when presenting it so
//! an in-flight status can switch languages without losing diagnostic details.

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub enum UiLanguage {
    #[default]
    #[serde(rename = "ENG")]
    English,
    #[serde(rename = "KOR")]
    Korean,
}

impl<'de> Deserialize<'de> for UiLanguage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // A new/unsupported UI-language value must not discard the user's game
        // folder, profile or package selections when the settings file loads.
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(
            match value.as_str().map(str::to_ascii_uppercase).as_deref() {
                Some("KOR" | "KO" | "KOREAN") => Self::Korean,
                _ => Self::English,
            },
        )
    }
}

impl UiLanguage {
    pub const fn code(self) -> &'static str {
        match self {
            Self::English => "ENG",
            Self::Korean => "KOR",
        }
    }

    pub const fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Korean => "한국어",
        }
    }

    pub fn text(self, source: &str) -> String {
        if self == Self::English {
            return source.to_owned();
        }
        if let Some((_, korean)) = KOREAN.iter().find(|(english, _)| *english == source) {
            return (*korean).to_owned();
        }
        // These two labels wrap another launcher-generated status. Translate
        // the recognized inner status while retaining unknown diagnostics.
        for (english, korean) in [
            ("Installed: ", "설치 상태: "),
            ("Configuration: ", "설정 상태: "),
        ] {
            if let Some(status) = source.strip_prefix(english) {
                return format!("{korean}{}", self.text(status));
            }
        }
        for (english, korean) in KOREAN_TEMPLATES {
            if let Some(arguments) = template_arguments(english, source) {
                return render_template(korean, &arguments);
            }
        }
        // Package IDs, URLs, paths, release notes and detailed native/OS errors
        // remain verbatim when they are not a recognized interface string.
        source.to_owned()
    }
}

// Reviewable exact strings: (English source, initial Korean translation).
const KOREAN: &[(&str, &str)] = &[
    ("Mods", "모드"),
    ("Tempest mods", "Tempest 모드"),
    ("Add mod…", "모드 추가…"),
    ("Mod added", "모드 추가됨"),
    ("Mod enabled", "모드 활성화됨"),
    ("Mod disabled", "모드 비활성화됨"),
    ("Mod removed", "모드 삭제됨"),
    ("Enable", "활성화"),
    ("Disable", "비활성화"),
    ("Remove", "삭제"),
    ("Actions", "관리"),
    (
        "Manage in Localization or Performance",
        "번역 또는 성능 탭에서 관리",
    ),
    ("Mods refreshed", "모드 새로고침됨"),
    ("Recovered", "복구됨"),
    ("Recover mods", "모드 복구"),
    ("Replace existing game files", "기존 게임 파일 교체"),
    ("Allow unverified DLL mods", "미검증 DLL 모드 허용"),
    (
        "Select a game folder to manage mods",
        "모드를 관리할 게임 경로를 선택하세요",
    ),
    ("No installed mods", "설치된 모드 없음"),
    (
        "Unable to read installed mods",
        "설치된 모드를 불러올 수 없음",
    ),
    ("Name", "이름"),
    ("Version", "버전"),
    ("Author", "제작자"),
    ("Status", "상태"),
    ("Enabled", "활성화됨"),
    (
        "Developer launch requires its original restart procedure",
        "개발용 실행은 기존 절차로 재시작하세요",
    ),
    ("Check for launcher updates", "런처 업데이트 확인"),
    ("Checking launcher updates…", "런처 업데이트 확인 중…"),
    ("No launcher release published", "출시된 런처 버전 없음"),
    ("Download and restart launcher", "다운로드 후 런처 재시작"),
    ("Downloading launcher update", "런처 업데이트 다운로드 중"),
    ("Restarting launcher", "런처 재시작 중"),
    ("Updating…", "업데이트 중…"),
    ("Restarting…", "재시작 중…"),
    ("Retry update", "업데이트 재시도"),
    (
        "Launcher update unavailable; see Diagnostics",
        "런처 업데이트 불가 · 진단 확인",
    ),
    ("PaladinsCat Launcher", "PaladinsCat 런처"),
    ("Interface language", "런처 언어"),
    ("Game installation", "게임 설치 경로"),
    ("Browse…", "찾아보기…"),
    ("Detect", "자동 검색"),
    ("Detecting…", "검색 중…"),
    ("Detected", "검색 결과"),
    ("Game language", "게임 언어"),
    ("English", "영어"),
    ("Korean", "한국어"),
    ("Profile", "프로필"),
    ("Play", "실행"),
    ("Launch platform", "실행 플랫폼"),
    ("Performance", "성능"),
    ("Localization", "현지화"),
    ("Language", "언어"),
    ("Translation", "번역"),
    ("Korean IME patch", "한국어 IME 패치"),
    ("Release", "릴리스"),
    ("Refresh", "새로고침"),
    ("Repository settings", "레포 설정"),
    ("Details", "상세 정보"),
    ("Diagnostics", "진단 정보"),
    ("Local path", "로컬 경로"),
    ("Copy log path", "로그 경로 복사"),
    ("Clear downloads", "다운로드 삭제"),
    ("Downloads cleared", "다운로드 삭제 완료"),
    (
        "Clearing downloads was interrupted",
        "다운로드 삭제가 중단되었습니다",
    ),
    ("Manifest", "매니페스트"),
    ("GitHub repository", "GitHub 레포"),
    ("Catalog URL", "카탈로그 URL"),
    ("Catalog", "카탈로그"),
    ("Translation package", "번역 패키지"),
    ("Import package…", "패키지 가져오기…"),
    ("Package manifest", "패키지 매니페스트"),
    ("No published package selected", "선택한 공개 패키지 없음"),
    ("Download package", "패키지 다운로드"),
    ("Apply & recovery", "적용 및 복구"),
    ("Apply / Update", "적용 / 업데이트"),
    ("Previous version", "이전 버전"),
    ("Restore originals", "원본 복원"),
    ("Recover interrupted operation", "중단된 작업 복구"),
    ("Korean IME compatibility", "한국어 IME 호환성"),
    (
        "No published IME package selected",
        "선택한 공개 IME 패키지 없음",
    ),
    ("Download IME package", "IME 패키지 다운로드"),
    ("Import IME package…", "IME 패키지 가져오기…"),
    ("IME package manifest", "IME 패키지 매니페스트"),
    ("Enable Korean IME fix", "한국어 IME 수정 활성화"),
    ("Update IME package", "IME 패키지 업데이트"),
    ("Recover IME operation", "IME 작업 복구"),
    ("Performance mod", "성능 모드"),
    ("Download performance package", "성능 패키지 다운로드"),
    ("Import performance package…", "성능 패키지 가져오기…"),
    ("Performance manifest", "성능 패키지 매니페스트"),
    (
        "Enable performance mod on next launch",
        "다음 실행 시 성능 모드 활성화",
    ),
    ("Use previous input version", "이전 입력 버전 사용"),
    ("Ready", "준비됨"),
    ("Preparing…", "준비 중…"),
    ("Working…", "작업 중…"),
    ("Game started", "게임 실행됨"),
    ("Launch requested", "실행 요청됨"),
    ("Refreshed", "새로고침 완료"),
    ("Package ready", "패키지 준비됨"),
    ("Done", "완료"),
    ("Operation failed", "작업 실패"),
    (
        "Match Game language to the installed package",
        "게임 언어를 설치된 패키지에 맞추세요",
    ),
    ("No game folder selected", "게임 폴더를 선택하지 않음"),
    (
        "Not inspected in CPU preview",
        "CPU 미리보기에서는 확인하지 않음",
    ),
    ("Disabled", "비활성화됨"),
    ("Not installed", "설치되지 않음"),
    ("Catalog not checked", "카탈로그를 확인하지 않음"),
    ("Catalog unavailable", "카탈로그를 불러올 수 없음"),
    ("Connecting to GitHub…", "GitHub에 연결 중…"),
    ("Catalog refreshed", "카탈로그 새로고침 완료"),
    (
        "Connected · no downloadable packages published",
        "연결됨 · 공개된 다운로드 패키지 없음",
    ),
    (
        "Connected · no localization packages published",
        "연결됨 · 공개된 현지화 패키지 없음",
    ),
    (
        "Connected · no IME packages published",
        "연결됨 · 공개된 IME 패키지 없음",
    ),
    (
        "Package downloaded/imported and verified. Ready to apply.",
        "패키지 다운로드/가져오기 및 검증 완료. 적용할 준비가 되었습니다.",
    ),
    (
        "IME package verified. Ready to apply.",
        "IME 패키지 검증 완료. 적용할 준비가 되었습니다.",
    ),
    ("Performance package ready", "성능 패키지 준비됨"),
    (
        "Download the selected performance package before launch",
        "실행 전에 선택한 성능 패키지를 다운로드하세요",
    ),
    (
        "Operation interrupted; use Recover if a transaction is pending",
        "작업이 중단되었습니다. 완료되지 않은 작업이 있으면 복구를 사용하세요",
    ),
    (
        "Downloading release manifest…",
        "릴리스 매니페스트 다운로드 중…",
    ),
    (
        "Recovering the previous verified installation…",
        "검증된 이전 설치 상태로 복구 중…",
    ),
    (
        "Restoring verified originals before switching packages…",
        "패키지를 변경하기 전에 검증된 원본을 복원 중…",
    ),
    ("Original game files", "원본 게임 파일"),
    (
        "Recovery required · interrupted installation",
        "복구 필요 · 설치가 중단됨",
    ),
    (
        "Recovered the previous installation",
        "이전 설치 상태로 복구 완료",
    ),
    (
        "Original game files restored and verified",
        "원본 게임 파일 복원 및 검증 완료",
    ),
    (
        "Local package · compatibility checked by file hashes",
        "로컬 패키지 · 파일 해시로 호환성 확인",
    ),
    (
        "Previous IME installation recovered and verified",
        "이전 IME 설치 상태 복구 및 검증 완료",
    ),
    (
        "IME selection already matches installation",
        "선택한 IME 패키지가 이미 설치되어 있습니다",
    ),
    (
        "Applying verified IME mod files…",
        "검증된 IME 모드 파일 적용 중…",
    ),
    (
        "IME mod disabled; verified files retained in the archive",
        "IME 모드 비활성화 완료. 검증된 파일은 보관되어 있습니다",
    ),
    (
        "Downloading pinned performance package v0.1.0…",
        "지정된 성능 패키지 v0.1.0 다운로드 중…",
    ),
    (
        "Performance package verified and prepared; no game files changed",
        "성능 패키지 검증 및 준비 완료. 게임 파일은 변경되지 않았습니다",
    ),
    (
        "Close Paladins before changing game files",
        "게임 파일을 변경하기 전에 Paladins를 종료하세요",
    ),
    (
        "Close Paladins before recovery",
        "복구하기 전에 Paladins를 종료하세요",
    ),
    (
        "Close Paladins before IME recovery",
        "IME 복구 전에 Paladins를 종료하세요",
    ),
    (
        "Close Paladins before IME replacement",
        "IME 패키지를 교체하기 전에 Paladins를 종료하세요",
    ),
    (
        "Recover the interrupted operation first",
        "먼저 중단된 작업을 복구하세요",
    ),
    (
        "Recover the interrupted IME operation first",
        "먼저 중단된 IME 작업을 복구하세요",
    ),
    (
        "Recover the interrupted localization/IME operation before launching",
        "실행 전에 중단된 현지화/IME 작업을 복구하세요",
    ),
    (
        "Choose/download/import a package first",
        "먼저 패키지 선택, 다운로드 또는 가져오기를 진행하세요",
    ),
    (
        "Download/import an IME package first",
        "먼저 IME 패키지를 다운로드하거나 가져오세요",
    ),
    (
        "Select the game folder containing ChaosGame",
        "ChaosGame 폴더가 있는 게임 폴더를 선택하세요",
    ),
    (
        "Select the game root containing Binaries, Engine and ChaosGame",
        "Binaries, Engine, ChaosGame 폴더가 있는 게임 설치 경로를 선택하세요",
    ),
    (
        "The selected Win64 game executable is missing",
        "선택한 Win64 게임 실행 파일을 찾을 수 없습니다",
    ),
    (
        "This package requires a newer PaladinsCat Launcher",
        "이 패키지에는 최신 PaladinsCat 런처가 필요합니다",
    ),
    (
        "Package belongs to a different game build",
        "이 패키지는 다른 게임 빌드용입니다",
    ),
    (
        "Insufficient disk space for staging and backups",
        "임시 파일 및 백업을 저장할 디스크 공간이 부족합니다",
    ),
    (
        "Another patch operation is using this installation",
        "다른 패치 작업에서 이 게임 설치 경로를 사용 중입니다",
    ),
    (
        "Game started during preparation; nothing applied",
        "준비 중에 게임이 실행되어 아무 변경도 적용하지 않았습니다",
    ),
    (
        "Game started; close it before recovery",
        "게임이 실행되었습니다. 복구 전에 게임을 종료하세요",
    ),
    (
        "Input rollback requires the selected performance package",
        "이전 입력 버전을 사용하려면 성능 패키지를 선택해야 합니다",
    ),
    (
        "Game path must be absolute",
        "게임 경로는 절대 경로여야 합니다",
    ),
    (
        "Use a public HTTPS GitHub catalog/release asset URL",
        "공개된 GitHub 카탈로그 또는 릴리스 파일의 HTTPS URL을 사용하세요",
    ),
    (
        "Installed receipt requires recovery",
        "설치 기록을 복구해야 합니다",
    ),
    (
        "Game launch is supported on Windows only",
        "게임 실행은 Windows에서만 지원됩니다",
    ),
    (
        "Launch plan changed during final validation",
        "최종 검증 중에 실행 설정이 변경되었습니다",
    ),
];

// Anchored launcher-generated templates. Placeholder contents are retained
// byte-for-byte: no global replacement in package names, paths or diagnostics.
// More specific forms precede wrappers whose final placeholder is unrestricted.
const KOREAN_TEMPLATES: &[(&str, &str)] = &[
    ("Update v{}", "v{} 업데이트"),
    (
        "Connected · {} translations · {} IME packages",
        "연결됨 · 번역 {}개 · IME 패키지 {}개",
    ),
    (
        "Connected · {} localization package",
        "연결됨 · 현지화 패키지 {}개",
    ),
    (
        "Connected · {} localization packages",
        "연결됨 · 현지화 패키지 {}개",
    ),
    ("Connected · {} IME package", "연결됨 · IME 패키지 {}개"),
    ("Connected · {} IME packages", "연결됨 · IME 패키지 {}개"),
    ("Pinned release · v{}", "지정된 릴리스 · v{}"),
    ("Enabled · {} · {}", "활성화됨 · {} · {}"),
    ("Local draft · {}", "로컬 초안 · {}"),
    ("Local · {}", "로컬 · {}"),
    ("Prepared: {} · local draft", "준비됨: {} · 로컬 초안"),
    ("Prepared: {}", "준비됨: {}"),
    ("Downloading {} / {} MiB", "다운로드 중: {} / {} MiB"),
    ("Preparing {}", "준비 중: {}"),
    ("Verifying {}", "검증 중: {}"),
    ("Installing and verifying {}…", "설치 및 검증 중: {}…"),
    (
        "Applied {} · all files verified",
        "{} 적용 완료 · 모든 파일 검증 완료",
    ),
    (
        "IME {} installed and verified · loaded by launcher on next launch",
        "IME {} 설치 및 검증 완료 · 다음 실행 시 런처에서 로드됩니다",
    ),
    (
        "Game dispatched (PID {}); fresh selected game observed (PID {}). Mod initialization is not yet verified.",
        "게임 실행 요청 완료 (PID {}). 선택한 게임의 새 실행 확인 (PID {}). 모드 초기화는 아직 확인되지 않았습니다.",
    ),
    (
        "Game dispatched (PID {}); selected game startup is still unconfirmed. Mod initialization is not yet verified.",
        "게임 실행 요청 완료 (PID {}). 선택한 게임의 시작과 모드 초기화는 아직 확인되지 않았습니다.",
    ),
    (
        "Game dispatch exited with code {}; no selected game was observed. See {}",
        "게임 실행이 종료되었습니다 (코드 {}). 선택한 게임의 실행을 확인하지 못했습니다. 로그: {}",
    ),
    (
        "Paladins is already running (PIDs {}). This launcher leaves every existing client intact; close the intended client before Play.",
        "Paladins가 이미 실행 중입니다 (PID {}). 기존 클라이언트는 유지됩니다. 실행 버튼을 누르기 전에 해당 클라이언트를 종료하세요.",
    ),
    (
        "Operation failed; previous installation restored: {}",
        "작업 실패. 이전 설치 상태로 복원했습니다: {}",
    ),
    (
        "{}; previous IME installation restored",
        "{}; 이전 IME 설치 상태로 복원했습니다",
    ),
    ("{}; recovery pending: {}", "{}; 복구 필요: {}"),
    ("{}. Recovery required: {}", "{}. 복구 필요: {}"),
    (
        "Could not save settings: {}",
        "설정을 저장할 수 없습니다: {}",
    ),
    (
        "Could not clear downloads: {}",
        "다운로드를 지울 수 없습니다: {}",
    ),
    ("Download cache cleanup: {}", "다운로드 캐시 정리: {}"),
    (
        "Receipt unavailable: {}",
        "설치 기록을 불러올 수 없습니다: {}",
    ),
    ("Unavailable: {}", "확인할 수 없음: {}"),
    ("GitHub catalog: {}", "GitHub 카탈로그: {}"),
    ("Saved translation: {}", "저장된 번역 패키지: {}"),
    ("Saved IME: {}", "저장된 IME 패키지: {}"),
    ("Saved performance package: {}", "저장된 성능 패키지: {}"),
];

fn template_arguments<'a>(template: &str, source: &'a str) -> Option<Vec<&'a str>> {
    let mut literals = template.split("{}");
    let mut rest = source.strip_prefix(literals.next()?)?;
    let mut arguments = Vec::new();
    let remaining: Vec<_> = literals.collect();
    for (index, literal) in remaining.iter().enumerate() {
        if index + 1 == remaining.len() {
            arguments.push(rest.strip_suffix(literal)?);
            rest = "";
        } else {
            let offset = rest.find(literal)?;
            arguments.push(&rest[..offset]);
            rest = &rest[offset + literal.len()..];
        }
    }
    rest.is_empty().then_some(arguments)
}

fn render_template(template: &str, arguments: &[&str]) -> String {
    let mut result = String::new();
    for (index, literal) in template.split("{}").enumerate() {
        if index > 0 {
            result.push_str(arguments[index - 1]);
        }
        result.push_str(literal);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize)]
    struct SavedSettings {
        game_folder: String,
        launch_language: String,
        #[serde(default)]
        ui_language: UiLanguage,
    }

    #[test]
    fn legacy_and_unsupported_ui_preferences_preserve_saved_game_choices() {
        let legacy = serde_json::json!({
            "game_folder": "C:/Games/팔라딘/Season 8.1",
            "launch_language": "KOR"
        });
        let settings: SavedSettings = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(settings.ui_language, UiLanguage::English);
        for unsupported in [
            serde_json::json!("FRA"),
            serde_json::Value::Null,
            serde_json::json!(42),
            serde_json::json!({"future": "preference"}),
        ] {
            let mut saved = legacy.clone();
            saved["ui_language"] = unsupported;
            let settings: SavedSettings = serde_json::from_value(saved).unwrap();
            assert_eq!(settings.ui_language, UiLanguage::English);
            assert_eq!(settings.game_folder, "C:/Games/팔라딘/Season 8.1");
            assert_eq!(settings.launch_language, "KOR");
        }
        let mut saved = legacy;
        saved["ui_language"] = serde_json::json!("kor");
        let settings: SavedSettings = serde_json::from_value(saved).unwrap();
        assert_eq!(settings.ui_language, UiLanguage::Korean);
        assert_eq!(
            serde_json::to_value(settings).unwrap()["ui_language"],
            "KOR"
        );
    }

    #[test]
    fn switching_status_language_retains_canonical_text_and_dynamic_payloads() {
        let source = "Prepared: paladinscat-ime-0-1-0 · local draft";
        assert_eq!(
            UiLanguage::Korean.text(source),
            "준비됨: paladinscat-ime-0-1-0 · 로컬 초안"
        );
        assert_eq!(UiLanguage::English.text(source), source);
        let source = "Game dispatch exited with code 7; no selected game was observed. See C:\\Games\\팔라딘\\launch-123.log";
        let korean = UiLanguage::Korean.text(source);
        assert!(korean.contains("(코드 7)"));
        assert!(korean.ends_with("C:\\Games\\팔라딘\\launch-123.log"));
        assert_eq!(UiLanguage::English.text(source), source);
        assert_eq!(
            UiLanguage::Korean.text("Installed: Original game files"),
            "설치 상태: 원본 게임 파일"
        );
        assert_eq!(
            UiLanguage::Korean.text("Configuration: Enabled · 0.1.0 · 8.1"),
            "설정 상태: 활성화됨 · 0.1.0 · 8.1"
        );
    }

    #[test]
    fn unknown_diagnostics_and_technical_error_contents_are_preserved() {
        for source in [
            "Access is denied. (os error 5)",
            "C:\\Games\\Ready\\Local · package\\Paladins.exe",
            "native ABI mismatch: expected [4, 9, 7, 1]",
            "unexpected Connected · 2 IME packages suffix",
        ] {
            assert_eq!(UiLanguage::Korean.text(source), source);
        }
        assert_eq!(
            UiLanguage::Korean.text("Could not save settings: Access is denied. (os error 5)"),
            "설정을 저장할 수 없습니다: Access is denied. (os error 5)"
        );
    }

    #[test]
    fn catalog_has_no_ambiguous_sources_or_unbalanced_placeholders() {
        let mut exact = std::collections::HashSet::new();
        for (english, korean) in KOREAN {
            assert!(exact.insert(english), "Duplicate source: {english}");
            assert!(!korean.is_empty(), "Missing Korean: {english}");
        }
        let mut templates = std::collections::HashSet::new();
        for (english, korean) in KOREAN_TEMPLATES {
            assert!(templates.insert(english), "Duplicate template: {english}");
            assert_eq!(
                english.matches("{}").count(),
                korean.matches("{}").count(),
                "Placeholder mismatch: {english}"
            );
        }
    }
}
