# Tempest mods

The **Mods** page reads the selected installation's `.tempest/mods/mods.json`
registry. It shows each mod's name, version, author and enabled state, including
entries installed by Tempest. **Refresh** rereads this local registry. State is
the recorded selection; it does not assert that a DLL has initialized in a game.

Each row has **Enable** or **Disable**, plus **Remove**. Disable restores original
game files (or removes files added by that mod) while retaining snapshots for
re-enabling. Enable reapplies the retained snapshot. DLL-only mods change the
registry flag used by the launch plan. Remove restores an enabled mod's game
files, removes its registry entry and recycles its snapshots and backups. Removing
an already disabled mod leaves current game files alone. Managed IME/performance
rows use their existing Localization/Performance controls.

**Add mod…** accepts Tempest V2 `.tempest`/`.zip` packages with `[mod]` metadata
in `manifest.toml`, and V1 `.upk`/`.pck` files. Imports use the same V1/V2 mod,
backup and INI-backup directories and PascalCase registry fields as Tempest.
Imported mods are registered enabled, as in Tempest. Existing metadata fields,
authors and unrelated entries are preserved. Existing IDs are refused.

V1 voice files use `ChaosGame/CookedPCConsole/English(US)`; other packages use
`ChaosGame/CookedPCConsole`. Native-package INI registration follows Tempest's
filename rules. V2 `files/` content is copied into the selected game's content
or configuration directories, with retained mod snapshots and original backups;
INI patches preserve BOM/newlines and support scalar and array operations.
`dlls/` remains in the V2 mod directory and is consumed by the existing guarded
launch plan. Adding a package never executes its DLLs.

Original non-INI files are replaced only when **Replace existing game files**
is selected. Another registered mod's file ownership and managed localization,
IME and performance boundaries remain protected. Archive traversal, Windows path
aliases, duplicate paths, symbolic links and oversized members are refused.
If `CHECKSUMS` is present, all included members must match it. Checksums establish
integrity, not publisher identity: OpenPGP signatures are not verified here, so
every DLL import requires **Allow unverified DLL mods**, including signed archives.

Changes require Paladins to be closed and share the localization installation
lock. Payloads and original bytes are staged under one bounded transaction with
a journal; the registry is committed last. An interrupted import restores the
previous files or keeps a recovery gate if restoration cannot be verified.
State changes and removal use the same journal, including reversible deletions.
**Recover mods** retries recovery. Play refuses an unresolved transaction.
Successful transaction staging is recycled rather than retained per import.

Launcher imports retain original/applied INI state so removal can reverse scalar,
array-addition, removal and clear operations without replacing unrelated settings.
Conflicting external edits block changes and preserve the file. Existing Tempest
V1 snapshots and V2 scalar/array-addition patches are supported; older V2 patches
using removal/clear operations without an original snapshot cannot safely be
reversed and return an actionable error. Missing payload snapshots likewise
require package reimport. All state-changing actions require Paladins to be closed.

Reference: Tempest `01081efe3cd966dbc8f84822640b10f12f3f4158`,
`Tempest.CLI/Mods/{ModV1Installer,ModV2Installer,ModCommands,IniPatcher}.cs` and
`TempestPathUtility.cs`. There is no build/runtime dependency on the Tempest tree
or its CLI. Opening/refreshing the list preserves the recorded enabled selections;
only the row actions change them.

Fixture checks cover V1/V2 payloads and metadata, UTF-16 INI patches, backups,
checksum/path/trust/ownership rejection, shared locking and interrupted-import
rollback. GUI checks use actual ENG/KOR egui frames. No live game modification,
DLL execution or gameplay acceptance is part of those checks.
Additional checks exercise disable/re-enable/remove, shared INI entries, external
edit conflicts, removal rollback, disabled-mod cleanup, actual row clicks and the
DLL launch plan after each registry state change.
