# PaladinsCat Launcher

[Download the launcher EXE](https://github.com/PaladinsCat/PaladinsCat-Launcher/releases/latest/download/paladinscat-launcher.exe).
Run it directly; no extraction is required.

## Use

Run `paladinscat-launcher.exe` directly. The launcher is delivered as an EXE;
no ZIP extraction is required for testing or normal use.

1. The launcher detects installed games from Steam libraries, Epic Games and Tempest. If several are found, choose one under **Detected**. Use **Browse** for a custom folder; a valid saved path stays selected.
2. Download a localization package and apply it while the game is closed.
3. Set **Game language** to **Korean (KOR)** to match the installed Korean package.
4. Choose any IME and performance options. Beside **Play**, select **PC (Hi-Rez)**, **Steam** or **Epic** for the game's login method, then select **Play**.

Use the language menu at the top right to choose **ENG** or **KOR** for the launcher. This choice is saved separately from the game's language and profile.

The version control beside that menu checks stable releases from this launcher's
own GitHub repo at startup. When an update is available, select **Update** to
download, verify and restart it. Settings and game files are preserved. The
previous EXE is restored if startup fails, then temporary files are cleared after
successful recovery. See [self-update](docs/self-update.md) for release requirements.

Localization packages come from [PaladinsCat-locales](https://github.com/PaladinsCat/PaladinsCat-locales).
Performance packages come from [PaladinsCat-mods](https://github.com/PaladinsCat/PaladinsCat-mods).
Use **Restore originals** to revert localization. Disable performance for the next launch, or select its retained input version. Close the game before changing installed IME files.

The launcher starts the validated Windows x64 game executable and loads enabled DLLs directly. No Tempest installation is required. Existing Tempest-compatible profile and enabled-mod settings are retained.

Open **Mods** to see the selected game's installed Tempest mods, versions,
authors and enabled states. **Add mod…** imports `.tempest`/`.zip` V2 packages
or legacy `.upk`/`.pck` files while Paladins is closed. File replacement and
unverified DLL imports each require their corresponding option. Existing registry
entries and original-file backups are preserved. See [mod imports](docs/tempest-mods.md).
Each mod row has **Enable/Disable** and **Remove** controls. Disabling restores
original game files and keeps the mod ready to enable again; removing also clears
its saved files and registry entry.

Local builds and check records are separate from an approved release. See
[CONTRIBUTING.md](CONTRIBUTING.md#artifact-format-and-release-version-control)
for EXE delivery and release version control.

## Development

Windows x64 and Rust are required. See [CONTRIBUTING.md](CONTRIBUTING.md) and [architecture](docs/architecture.md).

## Fork source

This launcher is a Rust adaptation of the launch workflow from [LowRezStudio/Tempest](https://github.com/LowRezStudio/Tempest), our fork source. Game dispatch and DLL loading are implemented in this repository. It does not include Tempest's Svelte frontend, C# CLI or Zig injector source.

The launch contract was reviewed at upstream revision `efe828dd28a48c4e1861ebf25a99cc16bd4b2425`. Changes are maintained independently; upstream improvements are reviewed and ported into Rust. This repository is not currently a GitHub-network fork of the upstream monorepo.

GPL-3.0-or-later. Tempest copyright and third-party notices are retained in [NOTICE.md](NOTICE.md).
