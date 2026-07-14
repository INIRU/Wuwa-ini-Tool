# Verification Traceability

This table is populated with concrete commands, tests, reviews, and evidence IDs
during implementation. A requirement is not complete because a neighboring test
passed.

| Requirement group | Planned evidence |
| --- | --- |
| REQ-PROD-* | version/license/i18n/icon review; Windows bundle inspection |
| REQ-INI-* | Rust golden/property tests; failure-injection tests; UI diff tests |
| REQ-CAT-* | schema tests; fixture validation; evidence review |
| REQ-CPU-* | Rust unit tests; Windows helper-process integration tests; security review |
| REQ-BACKUP-* | hash/retention/restore/failure tests; update preservation test |
| REQ-CACHE-* | allowlist/containment/reparse tests; preview-token tests; locked-file partial results; Windows fixture cleanup |
| REQ-UI-* | component tests; keyboard/accessibility review; desktop/min-size browser QA |
| REQ-UPD-* | updater unit/UI tests; tamper rejection; staging update; release asset review |
| REQ-OSS-* | repository file review; Issue/PR workflow validation |
| REQ-SAFE-* | bilingual copy review; first-run flow test; prohibited-scope security scan |

## Required command classes

- Frontend formatting, linting, typechecking, and unit tests.
- Rust formatting, Clippy with warnings denied, unit and integration tests.
- Dependency and supply-chain review.
- Production frontend and Tauri builds.
- Windows NSIS install/uninstall and helper-process integration tests.
- Git diff hygiene and repository documentation checks.
- Signed updater metadata, signature, tamper rejection, and upgrade-path checks.

## Local implementation gate — 2026-07-15

Tasks 1–12 are implemented locally at commit `a2fe6ed`. The final independent
integration review reported zero Critical, Important, or Minor findings.

| Gate | Result |
| --- | --- |
| Frontend unit/component/workflow tests | PASS — 10 files, 73 tests |
| TypeScript typecheck | PASS |
| Vite production build | PASS |
| Rust all-target/all-feature tests | PASS — 242 tests |
| `cargo fmt --check` | PASS |
| `cargo clippy --all-targets --all-features -- -D warnings` | PASS |
| Version, catalog, and icon policies | PASS — 17 options, 4 INI presets, 3 CPU presets; ICO 16/24/32/48/64/256 |
| Production dependency audit | PASS — 0 vulnerabilities |
| Git diff hygiene | PASS |
| Tauri configuration parse | PASS; host correctly reports missing macOS Xcode only |

The application gate includes the lossless Engine.ini workflows, raw full-file
paste/import, custom keys and portable profiles, byte-verified backups and
restore, all Windows priority choices, CPU Sets/affinity, adaptive Focus Mode,
independent WuWa/NVIDIA cache maintenance, bilingual safety copy, global signed
updater UI, and the original non-character `[T]` brand asset.

## Remaining Task 13 Windows release gate

The macOS development host cannot prove Windows runtime behavior or produce the
release-qualified NSIS installer. Publication remains blocked until all items
below pass on Windows CI and a clean Windows VM:

- Build the x64 NSIS installer and signed updater artifacts using protected
  GitHub secrets; verify required assets, hashes, SBOM, and signatures.
- Install, launch, single-instance, tray, uninstall, and reinstall the current-
  user package on a clean supported Windows system.
- Exercise native Kuro/Steam discovery, registry and reparse handling, native
  file dialogs, lossless apply/restore, and stale-preview rejection.
- Exercise priority classes, CPU Sets, compatible affinity, QoS normalization,
  protected-process exclusions, hot-core telemetry, crash recovery, and exact
  Focus Mode restoration with helper processes and real OBS/Discord/audio.
- Exercise separate WuWa and NVIDIA cache previews and cleanup with locked files,
  game-running refusal, junction/reparse attempts, and partial-result receipts.
- Verify signed update discovery, user prompt, real byte progress, same-handle
  install, interrupted resume/recovery, old-to-new upgrade, and tampered metadata
  or artifact rejection.
- Benchmark representative low-, mid-, and high-core-count CPUs before changing
  any default CPU preset; retain safe, optional defaults when results vary.
- Recapture final 1200×800 and minimum-window desktop states, including the
  global updater prompt, keyboard focus, tooltips, long Korean/English copy, and
  high-contrast behavior.
