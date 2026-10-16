# Contributing

Use Rust. The owner's agent workflow stays in the existing `main` checkout and
does not create branches, worktrees, forks or pull requests. Review the source
and run the checks below before an authorized publication. Remote protections
remain binding. External community contributions use owner-reviewed pull requests;
that procedure does not authorize an agent working for the owner to create one.

`rust-toolchain.toml` pins Rust 1.97.0 with Clippy/rustfmt. CI uses the same version;
upgrade the source and CI pins together under the applicable owner review workflow.

```text
cargo fmt --all --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release --bin paladinscat-launcher --bin paladinscat-launcher-cli --bin paladinscat-release
cargo run --locked --example verify_icon -- target/release/paladinscat-launcher.exe
```

Run `paladinscat-launcher --preview OUTPUT.png`, `--preview-ime OUTPUT.png` or `--preview-performance OUTPUT.png` for a CPU layout preview. These do not open or capture a desktop window. Unit tests use temporary fixtures; they do not install game packages or load downloaded/game DLLs. Loader tests use owned synthetic Rust DLL fixtures.

`--preview-details OUTPUT.png` and `--preview-details-kor OUTPUT.png` render the
expanded repo panel with a synthetic launch result to review compact ENG/KOR copy.

`--preview-prepared OUTPUT.png` and `--preview-prepared-kor OUTPUT.png` render the
saved setup with detected-install choices; `-kor` forces Korean. They read local store
records and the catalog without opening a desktop window or saving settings.
`paladinscat-launcher-cli --detect-games` lists the same valid install paths as
read-only JSON. Discovery tests use Steam/Epic/Tempest fixtures, including legacy
Steam libraries, moved/missing installs, duplicate paths and delayed UI results.

`--preview-mods OUTPUT.png` and `--preview-mods-kor OUTPUT.png` render the Mods
page with owned synthetic registry rows, without inspecting or modifying a game.
Mod imports and Enable/Disable/Remove actions use the fixtures described in
[Tempest mods](docs/tempest-mods.md), including file restoration, shared INI
entries, external-edit conflicts, transactional deletion rollback and row clicks.

`assets/paladinscat.png` supplies both the native window icon and the generated Windows executable icon. The build wraps the original PNG in an ICO under Cargo's output directory; do not maintain a second logo asset. `verify_icon` reads the compiled PE as data, requires Windows GUI subsystem 2, and compares its icon with the original PNG without starting the launcher. Both debug and release GUI builds use this subsystem; use the separate CLI binary for console workflows. Verify the exact EXE before delivering it for testing.

Do not commit downloaded packages, generated binaries, recordings, credentials or game files. Review and pin immutable package manifests and payload hashes before updating a release. Preserve existing game receipts and cache paths when changing product names. Keep launch selection distinct from verified runtime hook activation. No successful dry-run, build or CPU preview proves gameplay compatibility or performance.

The public graphics policy selects supported hardware before device allocation and refuses software fallback. An explicit per-user `graphics-policy.json` RTX 5080 pin retains strict vendor/device/name/PCI checks on the developer workstation. CPU checks do not establish live GPU routing.

## Artifact format and release version control

The launcher test and release artifact is always the directly runnable
`paladinscat-launcher.exe`. Local testing uses the existing
`target/release/paladinscat-launcher.exe`; provide the EXE path directly.
Do not make the owner unzip an archive to test each build. The optional CLI is
a separate `paladinscat-launcher-cli.exe` when requested. The authorized EXE-only
GitHub release uploads only the GUI EXE. Its release page provides the checksum,
tagged matching source, license/notices and `docs/dependency-sources.json` with
the exact source URL and SHA-256 of every locked dependency. Source remains
freely accessible separately from running the EXE. `cargo build --locked` obtains
these exact dependencies; `cargo vendor --locked --versioned-dirs vendor` prepares
an offline dependency source tree. The optional offline source ZIP emitted by the
preparer is not an uploaded asset for the EXE-only release.

Routine UI fixes, tests and documentation edits keep the existing package/PE
version and dist state. A passing check, changed hash, locked executable or
non-overwriting packager does not authorize a version bump or release cutover.
Do not create per-test version directories or replace/recycle previous staging
to make a new build appear current. Dist retention/cutover follows the human's
approved release scope; downloaded-cache cleanup is a separate feature.

For an authorized cutover, keep the existing `main` workflow and bind the approved
version to its source commit, EXE SHA-256, check results and publication state.
Local builds and staging are not Git tags, approved releases or GitHub publication.
For the current VM verification, the release build uses the stable
`target/vm/release` build directory while the workstation's
`target/release/paladinscat-launcher.exe` remains active. This is an ordinary
build output in this checkout, with the same package version and direct EXE
delivery; it does not create a dist or release cutover.
Preserve prior release evidence and staging until its disposition is authorized.

`paladinscat-release prepare-launcher` emits the GUI and optional CLI directly,
separate notices/checksums, a schema-2 release manifest, and a corresponding-source
ZIP with checked dependency sources. It does not create a launcher ZIP. Its clean
source, matching PE version and new `dist/<current-version>` gates remain binding;
do not run it or alter staging without an authorized release cutover.

For self-update, an approved GitHub stable release must use tag `v<major.minor.patch>`
and attach the exact asset `paladinscat-launcher.exe`. Verify GitHub's asset
`digest` is `sha256:<EXE SHA-256>` and its size matches the local artifact before
making the release latest. A tag, source archive, draft, prerelease, missing hash,
or ZIP alone cannot be installed by the updater. Release preparation never
publishes automatically. See [self-update](docs/self-update.md).

The native updater lifecycle check uses owned CPU-only Rust fixture EXEs:

```text
cargo build --locked --example updater_fixture --example verify_updater
cargo run --locked --example verify_updater -- target/debug/examples/updater_fixture.exe
paladinscat-launcher-cli --check-launcher-update
```

The lifecycle check covers successful replacement, startup failure and a missing
readiness signal, with actual Windows process handles and EXE replacement. It
never replaces the active launcher or starts a game. Synthetic fixture versions
are test data, not a package version change or published release.
