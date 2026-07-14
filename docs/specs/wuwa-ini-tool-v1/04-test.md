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
