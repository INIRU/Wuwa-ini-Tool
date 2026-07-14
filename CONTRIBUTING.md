# Contributing to Wuwa ini Tool

Thank you for helping improve Wuwa ini Tool. This repository welcomes focused
issues and pull requests that preserve its safety boundaries and evidence-first
approach.

By participating, you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md)
and to submit only material you are entitled to license under this project's
MIT terms.

## Before you start

1. Search existing Issues and pull requests.
2. Use the matching Issue form for a bug, feature, or option-evidence report.
3. For a large UI, architecture, safety, process-control, or file-format change,
   open a proposal before implementation.
4. Never use a public Issue for a vulnerability; follow
   [SECURITY.md](SECURITY.md).

Contributions must not add code injection, game-memory access, anti-cheat
hooks, drivers, IFEO persistence, arbitrary process control, unrestricted file
access, or techniques intended to bypass game controls. Do not add copied
configuration packs, prose, images, or code from a source without a compatible
license.

## Development setup

Use Windows 10/11 x64 for integration and installer work. Node.js 22+, npm,
Rust stable with the MSVC toolchain, Visual Studio C++ Build Tools, and WebView2
are required for the complete desktop build.

```powershell
npm ci
npm run tauri dev
```

Keep source identifiers, public contracts, non-obvious safety comments,
contributor documentation, commit messages, and GitHub automation in English.
Comments should explain why a safety or compatibility decision exists; avoid
restating obvious code.

## Option-evidence and catalog promotion protocol

An option is not promoted because it is popular, persists in a file, or appears
in an upstream Unreal reference. A promotion proposal must be reproducible and
must include all of the following:

- exact Wuthering Waves game version and region, when relevant;
- launcher (`Kuro`, `Steam`, or another clearly identified official path);
- CPU, GPU, RAM, Windows version, GPU driver, and relevant resolution/settings;
- exact INI section and option key;
- exact before and after values, including the removed/absent state;
- whether the claim concerns presence in the file or observed runtime behavior;
- FPS and 1% low only when relevant, with the exact capture tool, sampling
  method, run count, warm-up policy, and summary method;
- exact test area, route or workload, and duration for each run;
- artifacts such as frame-time captures, screenshots, videos, or analysis data;
- crashes, visual defects, regressions, and other negative results; and
- sanitized relevant logs or an explicit statement that no relevant log was
  produced.

Use repeated A/B/A or similarly controlled runs where practical. Change one
variable at a time. Report negative and inconclusive results. Do not present a
mobile observation as a PC default, an average FPS difference as a stutter fix,
or correlation as proof that the game consumed an option.

Catalog promotion requires maintainers to confirm schema validity, source
provenance, current-version relevance, reproducibility, and an evidence state.
Community reports normally remain `community_reported` or `experimental` until
independent PC runtime evidence exists. Options that are overridden, ignored,
or regressed must remain labeled accordingly. Built-in non-experimental presets
accept only options that satisfy the repository's verified promotion gate.

## Pull request workflow

Create a focused branch and keep unrelated formatting or refactors out of the
change. Add or update tests for behavior changes. Preserve user data and fail
closed at filesystem, process, update, and import trust boundaries.

Run the relevant checks before requesting review:

```powershell
node scripts/check-version.mjs
node scripts/validate-catalog.mjs
npm test -- --run
npm run typecheck
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
git diff --check
```

Windows-specific changes also need the repository-owned process fixture or a
Windows integration result. Never apply Realtime priority to a CI runner. Do
not include account tokens, usernames, absolute personal paths, full crash
dumps containing private data, updater private keys, or signing passwords in
tests, logs, screenshots, commits, or Issues.

The pull request description must explain the user-visible result, tests,
safety/privacy impact, rollback or recovery behavior, and residual risk. A
maintainer may ask for a smaller change or additional evidence before review.

## Releases

Only maintainers create releases. Release jobs run in a protected GitHub
environment, require the tag to match every package version, require protected
updater signing secrets, and create a draft release for verification. Never add
release secrets to a pull-request workflow or use `pull_request_target` to run
untrusted code.
