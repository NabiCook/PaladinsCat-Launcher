# Notices

## Tempest

Mod import/registry and INI-patch contracts were reviewed in `ModFormatDetector.cs`,
`ModRecord.cs`, `ModV1Installer.cs`, `ModV2Installer.cs`, `IniPatcher.cs`,
`ModCommands.cs` and `TempestPathUtility.cs` at Tempest revision
`01081efe3cd966dbc8f84822640b10f12f3f4158`. They are adapted into independent Rust
imports with bounded archive reads, path/ownership checks and a recovery journal.

Fork source: https://github.com/LowRezStudio/Tempest

Copyright (C) 2025-2026 LowRezStudio contributors.

Tempest is licensed under GNU GPL version 3 or, at your option, any later version. The Rust launch workflow is derived from the contracts in `Tempest.CLI/Launcher/LauncherCommands.cs`, `LauncherUtility.cs` and `Tempest.CLI/Mods/ModCommands.cs` at revision `efe828dd28a48c4e1861ebf25a99cc16bd4b2425`. The full GPL text is in LICENSE.

The native loading contract was also reviewed in `Tempest.CLI/inject/main.zig` at that revision. This repository's independently written Rust loader uses Unicode paths, remote owning-module RVA/image verification and bounded waits in place of the reviewed upstream ANSI path and infinite wait. No upstream C#, Zig injector source or Tempest binaries are copied or redistributed; the standalone launcher does not require a Tempest installation. The upstream copyright and GPL attribution remain retained.

The login-choice pass-through contract was also reviewed at Tempest revision
`1fb304e1c2163f63b91ec51a159c8415a5aedf36` in
`Tempest.Launcher/src/lib/components/{library/InstanceWizard,onboarding/OnboardingPage}.svelte`,
`Tempest.Launcher/src/lib/core/launch.ts` and
`Tempest.CLI/Launcher/LauncherCommands.cs`. Steam adds `-steam`, Epic adds `-epic`
and Hi-Rez adds no login flag. The dropdown, saved setting and typed Rust argument
builder are implemented here; no upstream frontend or CLI source is redistributed.

## PaladinsCat

Copyright (C) 2026 PaladinsCat contributors. Localization installer and GUI source were contributed from WatchCat. Native game-hook implementations and recordings are not included. Mod release constants and public package contracts originate in PaladinsCat-mods; its MIT notice is retained in `licenses/PaladinsCat-mods-MIT.txt`. Downloadable native binaries retain their own package license notices.

## Fonts and artwork

Inter is distributed under SIL Open Font License 1.1; see `assets/Inter-LICENSE.txt`. The PaladinsCat logo is project artwork. The Windows Korean fallback font is read from the user's system and is not redistributed.

The language selector's Earth/Globe2 outline is adapted from Lucide Icons under the ISC license; see `licenses/Lucide-ISC.txt`.
