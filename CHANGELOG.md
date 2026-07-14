# Changelog

All notable changes to this project will be documented in this file. The format
is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Continue Windows clean-VM validation toward the stable `1.0.0` release.

## [1.0.0-beta.2] - 2026-07-15

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

[Unreleased]: https://github.com/INIRU/Wuwa-ini-Tool/compare/v1.0.0-beta.2...HEAD
[1.0.0-beta.2]: https://github.com/INIRU/Wuwa-ini-Tool/releases/tag/v1.0.0-beta.2
[1.0.0]: https://github.com/INIRU/Wuwa-ini-Tool/releases/tag/v1.0.0
