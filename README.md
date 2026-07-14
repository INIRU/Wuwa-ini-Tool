# Wuwa ini Tool

[![CI](https://github.com/INIRU/Wuwa-ini-Tool/actions/workflows/ci.yml/badge.svg)](https://github.com/INIRU/Wuwa-ini-Tool/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-gold.svg)](LICENSE)

Wuwa ini Tool is an unofficial, open-source Windows desktop utility for
managing Wuthering Waves `Engine.ini` settings and temporary per-session CPU
preferences. It is built with Tauri v2, Rust, React, and TypeScript.

한국어 안내는 [README.ko.md](README.ko.md)를 참고하세요.

> [!WARNING]
> This project is not affiliated with, endorsed by, or supported by Kuro Games.
> Configuration and process changes can cause crashes, lost settings, worse
> performance, or account action. Performance and account safety are not
> guaranteed. Review the [bilingual disclaimer](DISCLAIMER.md) before use.

## Safety model

- Edits only the validated
  `Client/Saved/Config/WindowsNoEditor/Engine.ini` associated with the selected
  game installation. It does not create or use `UserEngine.ini` as a bypass.
- Shows a diff and requires confirmation before writing.
- Creates and verifies a byte-for-byte backup before every apply or restore.
- Preserves unrelated sections, comments, ordering, line endings, supported
  encodings, unknown keys, and duplicate-key evidence.
- Treats imported profiles and pasted INI documents as untrusted input.
- Uses bounded, allowlisted cache-cleanup roots and never follows reparse
  points.
- Does not inject code, access game memory, hook anti-cheat, install a driver,
  change IFEO, or bypass game technical controls.

## Features

- Bilingual Korean/English interface with system, light, and dark themes.
- Conservative Vanilla, Balanced, Performance, and Visual Quality presets.
- Bilingual option evidence, support states, warnings, and source links.
- Advanced full-document paste/import plus custom section/key/value entries.
- Unified and split diffs, immutable original backup, pinned backups, and
  integrity-checked restore.
- Portable profile import/export without machine paths, device identifiers, or
  backup history.
- Windows priority classes and topology-aware CPU Set or advanced affinity
  selection with readback verification.
- Optional, default-off Focus Mode with protected communication, capture,
  audio, foreground, and system processes.
- Separate previews for WuWa and current-user NVIDIA shader-cache cleanup.
- Signed GitHub updater checks; installation always requires user approval.

An option being present in `Engine.ini` does **not** prove the game used it.
Community-reported and experimental options remain clearly labeled and are not
promoted into automatic presets without reproducible evidence.

## Current status

Version `1.0.0` is the first planned public release. Until the repository has a
published, verified GitHub Release, build from source or use a draft artifact
only for testing. Release automation intentionally fails if the protected
updater signing secrets or committed updater public key are missing.

## Build from source

Prerequisites:

- Windows 10 or 11 x64
- Node.js 22 or later and npm
- Rust stable with the MSVC Windows toolchain
- Visual Studio Build Tools with Desktop development with C++
- WebView2 (normally included with supported Windows versions)

```powershell
git clone https://github.com/INIRU/Wuwa-ini-Tool.git
cd Wuwa-ini-Tool
npm ci
npm run tauri dev
```

Run the release-equivalent local checks:

```powershell
node scripts/check-version.mjs
node scripts/validate-catalog.mjs
npm test -- --run
npm run typecheck
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
```

Build an NSIS installer with `npm run tauri build -- --bundles nsis`. A public
release also requires protected updater signing material; local unsigned builds
are not substitutes for official updater artifacts.

## Contributing and support

- Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.
- Use the structured [Issue forms](https://github.com/INIRU/Wuwa-ini-Tool/issues/new/choose)
  for bugs, feature requests, and option evidence.
- Read [SUPPORT.md](SUPPORT.md) for troubleshooting boundaries.
- Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md).

Do not submit copied text, images, configurations, or code unless its license is
compatible with this MIT-licensed repository. Public reports are evidence, not
permission to copy their content.

## License

Source code is licensed under the [MIT License](LICENSE). The MIT license and
the project disclaimer are separate documents; neither changes the game's
terms or third-party rights.
