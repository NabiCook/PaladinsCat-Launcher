# Standalone launcher updates

The small version button in the header checks this launcher's own
[stable releases](https://github.com/NabiCook/PaladinsCat-Launcher/releases).
A background check runs once at startup; selecting the version checks again.
When a newer version exists, **Update v…** downloads and restarts the launcher.
Updates require that click; they do not interrupt a running package operation.
ENG/KOR labels follow the launcher's language setting.

The updater downloads the complete new EXE. It does not download the source ZIP,
re-fetch localization/performance packages, or use Git diffs. The EXE stays at
its current path. Settings, profile, game folder and installed game files are
preserved. Its directory must be writable; failures leave the current launcher
running and appear under collapsed **Diagnostics**. There is no elevation prompt
or installer prerequisite.

## Release contract and trust

The fixed endpoint is the GitHub REST API
`repos/NabiCook/PaladinsCat-Launcher/releases/latest`, independent of the
editable localization repo/catalog. No token or browser session is needed.
GitHub documents the [stable latest-release endpoint](https://docs.github.com/en/rest/releases/releases#get-the-latest-release)
and [asset SHA-256 digest](https://docs.github.com/en/rest/releases/assets#get-a-release-asset).
Only published stable `vMAJOR.MINOR.PATCH` releases with a higher numeric version
are eligible. Equal-version rebuilds and downgrades are never installed.

The required asset is exactly `paladinscat-launcher.exe`, with GitHub's
`sha256:` digest, declared size no larger than 128 MiB, and an x64 PE version
matching the tag. Metadata/body sizes and connection/read/download waits are
bounded. A missing checksum, malformed release, wrong repository URL, partial
body, changed hash or wrong PE version is rejected before restart.

Trust is GitHub HTTPS plus write access to this specific repository's releases;
the checksum verifies bytes against GitHub metadata. It is not a publisher code
signature or independent signing-key system. Protect release-write access.
The release preparer emits direct EXEs, checksums, notices, source and dependency
sources separately, and never publishes or increments the version itself.

The first authorized stable release is v0.3.8. The GitHub release contains only
`paladinscat-launcher.exe`; matching source, license/notices and exact dependency
source links remain available in the tagged public repository. See
[v0.3.8 release verification](release-0.3.8.md). Before the first release existed,
the API's 404 correctly mapped to **No launcher release published**.

## Replacement and recovery

One fixed sibling transaction is reserved beside the running EXE:
`paladinscat-launcher.update.json`, `.update.exe`, `.updater.exe`, `.previous.exe`
and the two acknowledgement JSON files. Each full filename starts with
`paladinscat-launcher`; no per-version directories accumulate.
Atomic journal writes use the single fixed `.update-journal.tmp` sibling too;
an incomplete or foreign journal staging file is retained for recovery, without
creating another temporary name on each retry.

The helper is a verified copy of the current EXE, running Rust before any GPU
allocation. It opens the exact parent PID and creation time, verifies image and
payload hashes, and signals that it is armed. Only then does the GUI close
normally. The helper waits for that process handle to exit and uses Windows
`ReplaceFileW` with one rollback copy. The new launcher must verify its renderer
and build its first UI frame, then acknowledge its PID, creation time and image
hash. An immediate exit or missing acknowledgement restores and restarts the
previous EXE. No game or downloaded DLL is executed by this process.

The next startup clears the verified temporary files after the helper exits.
Deletion uses the existing silent Recycle Bin mechanism, without a permanent
delete fallback. An interrupted download is identified by its recorded Windows
file identity. An unexplained target, backup or file identity is preserved for
recovery rather than removed. After an interrupted machine, a pinned new image
that starts successfully can commit the existing transaction; the old backup
remains until that acknowledgement is saved. If Windows cannot complete either
replacement, the journal and previous EXE remain available for manual recovery.

Internal developer launches with `--vm-renderer-proof` retain their original
proof/host verification procedure; they can check releases but do not use the
normal self-restart path. This does not change public hardware selection or
the workstation's RTX 5080 pin.

## Verification

Unit tests cover release provenance, missing/invalid hashes, duplicate or ZIP
assets, prereleases, version ordering, corrupt/truncated/oversized payloads,
wrong PE versions, ownership and cleanup. The owned Rust EXE lifecycle check
uses actual Windows process waits and replacement for success, startup failure,
readiness timeout and a locked EXE that Windows refuses to replace. It verifies
the final EXE, preserved settings and absence
of retained transaction files. These are CPU fixture checks; a real public
release download/install requires an approved release to exist.

The 2026-10-15 source verification passed 156 tests (two existing opt-in fixtures
ignored), strict all-target Clippy, formatting, the Windows x64 release build,
embedded icon verification and public source audit. Actual ENG/KOR egui previews
were checked at the default size. The four owned EXE lifecycle checks passed
with settings preserved and no retained transaction files. A parallel cache-test
fixture collision was repaired by adding a monotonic ID to its directory name.
The separate WatchCat root check remains blocked by existing formatting in
`agent/examples/raw_inject_bench.rs` and `raw_read_bench.rs`; later root checks
did not run. No version change, tag, GitHub release or dist replacement occurred.
