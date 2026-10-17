# Launcher architecture

The test and release artifact is the directly runnable `paladinscat-launcher.exe`.
Matching source and legal notices are linked from the release page. A launcher ZIP
requiring extraction is not the product delivery format. See CONTRIBUTING.md for
the release/version contract; release preparation emits direct EXEs with separate
legal and source companions. Routine changes preserve
the existing version and staging until an authorized release cutover.

The native window starts at 880 × 660 logical pixels. CPU layout previews use
that same default size. Taller tabs and expanded sections use the existing
vertical scroll area; the window remains resizable.

`self_update` checks the launcher's own GitHub stable-release API on a separate
worker. It consumes the fixed direct EXE asset, with GitHub's SHA-256/size and a
matching x64 PE version. A copy of the existing EXE runs the internal Windows
helper before renderer allocation, waits for the exact original PID/creation
time, replaces the EXE with one backup, then requires a PID/hash-bound first-frame
acknowledgement. Failure restores the previous EXE. The normal startup worker
clears only the transaction's owned sibling files after the helper exits.
Settings, game data and package caches have separate ownership. See
[self-update](self-update.md) for trust, crash recovery and verification.

The GUI and installer moved from WatchCat's localization-patcher into this standalone public Rust crate. No build dependency reaches outside this repository. Native hook source stays in WatchCat. Locale authoring stays in PaladinsCat-locales; this launcher consumes its reviewed catalog and immutable release assets. Performance payloads stay in PaladinsCat-mods. The launcher data directory is `%LOCALAPPDATA%/PaladinsCat/Launcher` (renamed from `Localization` in 0.3.11 to reflect that it holds all launcher data, not just localization packages). The `.watchcat-localization` game receipts keep their name for compatibility.

Windows resolves the LocalAppData known folder with package redirection disabled instead of trusting a host's environment override. `paladinscat-launcher-cli --data-dir` prints the requested location without creating files. Windows can still virtualize file operations inherited from a packaged development host; CPU checks in that host recorded the effective cache path separately. Normal unpackaged use retains the existing patcher's settings location. This lookup does not claim to escape an MSIX container.

CLI `--dry-run GAME_ROOT` and `--launch GAME_ROOT` accept optional
`--cache-root EXISTING_DIRECTORY` for performance imports and `launch-logs`.
The explicit root is canonicalized and must already be a directory; links,
reparse points (including parent components), duplicate options and missing
values are refused. Its position relative to `--performance MANIFEST` does not
matter. Omitting it keeps the default data directory and existing cache ownership
and cleanup checks.

## Launch

Game-folder detection runs on a separate background worker, independently of
GitHub/catalog availability. It reads Steam's registered client roots and
modern/legacy `libraryfolders.vdf`, then Paladins app `444090` manifests in every
listed library, including custom libraries on other drives. It also reads Epic's
ProgramData `.item` install manifests and prepared Tempest instances from the
user's `com.lowrezstudio.tempest/settings.json`. Windows known folders retain
redirected user/shared locations. Provider records are read only, size bounded
and ignored individually when invalid; detection never recursively scans drives.

Candidates must contain `Binaries/Win64/Paladins.exe` and the `Binaries`, `Engine`
and `ChaosGame` directories. Canonical paths deduplicate installs across providers.
A valid saved path stays selected. An empty or missing saved path fills
automatically when exactly one installation exists; multiple installs appear in
the **Detected** dropdown for explicit selection. **Detect** refreshes that list;
the editable path and **Browse** remain available for custom installs. A late
result cannot overwrite a newer manual edit. Detect, Detected and Browse share
the path row and a common control height. Detection does not change Play's
existing version/fingerprint checks or install packages. The read-only CLI
`--detect-games` prints the same candidates without saving a selection.

**Play** validates the selected game, installed IME receipt, enabled performance package and native system fingerprints, then launches the Windows x64 game executable and loads the planned DLLs through this repository's Rust backend. No installed Tempest CLI is required and no console is opened. The selected profile, three-letter game locale and compatible native defaults are passed to the game. Only one performance input variant is selected. The launcher does not close an existing client.

New settings and CLI operations default to language `INT` and profile `PaladinsCat`. The GUI retains explicit saved language/profile choices; changing a default never migrates existing profile files. A profile is the game's `-homedir` argument, not a localization package selector. A retired `tempest_cli` key in saved JSON is ignored; existing profile, language, prepared packages and next-launch selections remain selected.

Replay libraries are profile-specific. See [replay profiles](replay-profiles.md) for storage, playback and cross-profile handling.

The saved `launch_platform` dropdown beside Play selects the login method:
**PC (Hi-Rez)** adds no platform flag, **Steam** passes `-steam`, and **Epic**
passes `-epic`. Only the chosen platform flag enters the game's argument list;
the old unconditional `-nosteam` is removed. Existing language, profile, native
defaults, DLL selection and compatibility checks remain in the same launch plan.
New and legacy settings default to PC; unsupported saved values fall back to PC
without discarding other preferences. Install detection does not override this
explicit login choice. The CLI exposes the same builder through
`--login-method pc|steam|epic` on dry-run and launch operations.

This follows Tempest's `InstanceWizard.svelte` / `OnboardingPage.svelte` mapping
and `core/launch.ts` forwarding through CLI escaped arguments at revision
`1fb304e1c2163f63b91ec51a159c8415a5aedf36`; see NOTICE.md. Here the selected flag
passes directly to the Windows process as its own argument, without a shell or
an installed Tempest dependency. The selection does not start a Steam/Epic
launcher, collect credentials or assert a successful platform login.

Canonical paths remain the internal identities for containment, receipts, deduplication and read-only file pins. External executable, game-root and DLL paths use ordinary Win32 drive/UNC syntax. Conversion must resolve back to the same file; unsupported device namespaces and conversions that redirect a trailing-dot/space component are refused. The game receives its `Binaries/Win64` as its working directory. The two required startup INIs are checked under the selected root, hashed, pinned and compared again before dispatch. No game configuration is rewritten. See [0.3.1 verification](verification-0.3.1.md) for the historical startup-failure evidence and transport-path boundary.

The launcher retains the compatible enabled V2 registry at `.tempest/mods/mods.json`; directory presence alone does not enable a mod. It loads safe enabled registry DLLs together with verified selected IME/performance packages, deduplicating matching files and refusing conflicting managed entries. Registry metadata and ownership remain intact. A registry entry alone does not establish that its payload was approved as a managed package. CLI dry-run output exposes `game_executable` and the full `load_dlls` list while reporting `hooks_verified: false`.

`src/native_launch.rs` implements the Windows loader in Rust. It writes a UTF-16 DLL path into the newly spawned game, resolves the owning system module and export RVA against the child's matching image, then invokes `LoadLibraryW` and verifies the exact loaded file through the child module inventory. An import that already loaded the selected DLL is deduplicated. Selected loads run in order after the compatible one-second startup delay, with ten seconds per load and a thirty-second overall loading deadline. Failed loads clean up only the new child with a bounded five-second exit wait. Successful dispatch releases its handles without a kill-on-close job, allowing the game to survive launcher/helper exit.

No background service is required. Configuration, process discovery and loaded module presence are distinct from verified hook behavior; launch dispatch does not prove that gameplay works. Turning off performance changes the next launch; the running client is not modified. IME enable/disable retains its existing guarded file transaction and recovery journal.

## Package and update contracts

The top-right launcher language menu follows the frontend's LanguageMenu design: globe, language code and chevron, with a right-aligned dark popup and selected-row accent. It supports ENG and KOR. `ui_language` is an independent saved preference; old or unsupported values fall back to English without discarding game/profile/package settings. Changing it translates the current labels and statuses immediately, including operations already in progress, and never changes the game or package locale.

`src/i18n.rs` contains the first-pass Korean wording for owner review. Status text remains canonical internally and is translated at presentation. Structured startup failures retain separate captions; dynamic IDs, paths, versions and detailed native/OS errors stay intact. Korean glyphs use the existing Windows font fallback. CPU preview flags support Korean sections and the open language menu; these do not launch a desktop window.

Performance is the first top-level menu option and opens by default. Localization
contains its package-language selector. Selecting `KOR` exposes Translation and
Korean IME patch subpages; other locales show only translation controls. The
language selector shares the Localization navigation row and remains visible on the IME subpage. Changing away from Korean
returns that subpage to Translation without changing the installed IME receipt
or any next-launch selection.

`src/sources.rs` owns repository identities and public source URLs shared by
the GUI, package downloader and release assembler. Repository links, settings
and publication status are in a bottom section, collapsed by default. Localization and Korean IME currently share the locale
catalog, but each displays only its own release array. Performance uses the
pinned mods release and never inherits a locale-catalog connection status.

A saved Catalog URL overrides the repository's default rolling `main` catalog.
A release-asset catalog URL is an intentional snapshot: Refresh reloads that
snapshot and cannot advance it. Latest-package discovery uses the rolling catalog;
publishing payloads alone is insufficient until `game-client/catalog.json` is
also published on locale `main`. Verify the live rolling response after that push,
since GitHub raw-content caches can briefly retain the preceding catalog.

The game language is a dropdown, independent of the launcher and package
languages. INT and KOR are offered; an existing saved locale code remains
selectable. The footer uses fixed, concise localized states. Launch results
show only Game started or Launch requested; raw process IDs, verification prose,
paths and native errors require opening Diagnostics inside the bottom section.
Expanding Repository settings shows the selectable local data/downloads path
and, after a launch, Copy log path. It does not repeat the footer status or expose
backend launch prose. Korean repository labels use 레포.
Backend/CLI diagnostics retain their full payloads. Headless interaction checks
enforce the default disclosure boundary in both interface languages.

Legacy repository/catalog settings and prepared package paths are retained.
Changing the catalog source invalidates its published lists while retaining
verified prepared packages. Downloading does not remove published entries.
Refreshing the catalog and switching tabs cannot turn a locale response into
performance availability. Pinned performance source information is distinct
from a verified downloaded package or runtime hook.

Localization and IME use the existing versioned catalog schema, decoded hashes, game fingerprint and receipt/journal rollback. The performance v0.1.0 manifest, archive and source revision are pinned from the actual published assets. Archive extraction uses an exact filename allowlist, member hashes, sizes and checksums; executable contents are never invoked during validation. Source tests and cached packages do not prove performance gains or combined IME compatibility.

Game or Windows updates that break native pins refuse the affected mod. A newly supported build requires a reviewed package/profile. New performance releases require reviewed immutable pins in launcher source; automatic unreviewed latest-release switching is not implemented. Locale catalogs can advance independently through their governed publication workflow.

## Upstream maintenance

Tempest is the documented fork source. Keep its pinned upstream revision and GPL notice with the ported launch contracts. Review changes to upstream argument defaults, metadata layout and DLL loading before upgrading. The standalone Rust backend requires neither a sibling Tempest checkout nor an installed Tempest application. Historical verification documents describe their tested version and are retained as evidence.

## Verification boundaries

Version 0.3.6 passed 84 all-target tests, formatting, strict Clippy, the release build and compiled icon verification. Owned synthetic Rust DLL tests exercise Unicode paths, import deduplication, failed/timed-out loads and surviving process/handle lifetimes. Settings migration coverage checks that the retired Tempest path does not reset existing choices.

One authorized Windows VM launch used the same backend as **Play** through `examples/interactive_launch_test.rs` in the logged-on desktop session. No Tempest application was installed or running. The selected game started, `WolperReplay.dll` appeared in its module inventory, startup/render-thread evidence was present, and the game remained alive for more than three minutes after helper and remote-command exit. A later inventory found the game absent before VM power-off; its exit cause is unknown. See [0.3.6 verification](verification-0.3.6.md). These observations establish standalone startup and module loading for that instance.

The GUI window was not opened. CPU checks and the VM game launch do not establish the GUI's live GPU routing. Authentication, replay behavior, gameplay, reporting, performance gains, combined IME/performance compatibility, portable IME runtime status and live disable remain unverified.

The public GUI selects a Windows DirectX 12 hardware adapter before device allocation, preferring discrete over integrated hardware and using a stable identity order. Software rendering is refused. The exact selected adapter is supplied to the renderer and its retained identity is checked at initialization. A per-user `graphics-policy.json` beside the launcher settings can require the developer's RTX 5080: `{"schema_version":1,"selection":"rtx_5080"}`. That policy requires the NVIDIA vendor/device, RTX 5080 name and physical PCI identity, and refuses missing or ambiguous hardware. No policy file defaults to `public_hardware`; invalid policy files fail closed. The selected policy and actual adapter identity are recorded in `graphics-device.json`. Machine policy files are not distributed in release packages. The explicit VM proof mode remains separate; these source rules do not establish a new live host or VM verification.
