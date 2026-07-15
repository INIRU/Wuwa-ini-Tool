# Changelog

All notable changes to this project will be documented in this file. The format
is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Continue Windows clean-VM validation toward the stable `1.0.0` release.

## [1.0.0-beta.6] - 2026-07-15

### Changed

- Pinned Cargo's default run target to the real `wuwa-ini-tool` desktop binary
  so Tauri cannot package the disposable Windows process fixture.
- Made release checksum entries use the exact GitHub asset names and added a
  repository policy check for the required Cargo default target.

### Fixed

- Restored the generic read-only flag on Windows when a backup does not carry
  raw Windows file attributes.
- Made focus-exclusion and Steam-library fixtures use valid host-specific
  Windows paths and VDF escaping.

## [1.0.0-beta.5] - 2026-07-15

### Changed

- Pinned Windows CI and release jobs to the stable Windows Server 2022 image
  instead of the preview Visual Studio environment behind `windows-latest`.
- Added Windows test-binary execution to the regular CI gate so loader/API
  incompatibilities fail before a release tag is created.
- Embedded the Common Controls v6 activation manifest into Windows Rust test
  executables so `TaskDialogIndirect` resolves the same way as in Tauri builds.

### Fixed

- Routed first-time metadata writes through the create-new transaction instead
  of the Windows replace-existing path, preserving no-clobber semantics.

## [1.0.0-beta.4] - 2026-07-15

### Fixed

- Replaced unstable Windows metadata extensions with stable Win32 handle
  inspection for backup identity and hard-link/reparse-point validation.
- Declared the serialized PDH telemetry sampler as thread-movable so Tauri can
  safely own it behind the runtime supervisor mutex.
- Completed the first clean GitHub-hosted Windows x64 Tauri and NSIS build.

## [1.0.0-beta.3] - 2026-07-15

### Added

- First public Windows beta with lossless `Engine.ini` diff/apply, full-document
  import, custom entries, portable profiles, verified backups, and restore.
- Optional priority, CPU Set, affinity, QoS, and adaptive Focus Mode controls
  with protected communication, capture, audio, and foreground applications.
- Separate WuWa and NVIDIA shader-cache maintenance previews.
- Signed updater artifacts, bilingual safety guidance, and the original `[T]`
  application icon.

### Known limitations

- This beta is an unofficial community tool and does not guarantee improved
  performance or account safety.
- Engine options may be ignored or regress after a game update.
- NVIDIA cache cleanup affects shared current-user driver caches and can cause
  temporary shader recompilation stutter.
- Hardware-matrix and clean-Windows runtime evidence is still being collected
  before the stable release.

## [1.0.0] - TBD

### Added

- Lossless, diff-first `Engine.ini` editing with verified backup and restore.
- Bilingual evidence catalog, conservative presets, custom entries, and
  portable profile sharing.
- Windows CPU Set, affinity, priority, and optional adaptive Focus Mode controls.
- Separate WuWa and NVIDIA shader-cache maintenance previews.
- Signed-update integration, release governance, and Windows NSIS packaging.

`1.0.0` remains unreleased until the clean-Windows, updater-signature,
installer, backup-preservation, and release-asset gates pass.

[Unreleased]: https://github.com/INIRU/Wuwa-ini-Tool/compare/v1.0.0-beta.6...HEAD
[1.0.0-beta.6]: https://github.com/INIRU/Wuwa-ini-Tool/releases/tag/v1.0.0-beta.6
[1.0.0-beta.5]: https://github.com/INIRU/Wuwa-ini-Tool/releases/tag/v1.0.0-beta.5
[1.0.0-beta.4]: https://github.com/INIRU/Wuwa-ini-Tool/releases/tag/v1.0.0-beta.4
[1.0.0-beta.3]: https://github.com/INIRU/Wuwa-ini-Tool/releases/tag/v1.0.0-beta.3
[1.0.0]: https://github.com/INIRU/Wuwa-ini-Tool/releases/tag/v1.0.0
