# Wuwa ini Tool 1.0.0 Design

Date: 2026-07-14  
Status: Approved  
License: MIT  
Target: Windows 10/11 x64

## Purpose

Wuwa ini Tool is an unofficial, open-source Tauri v2 desktop utility for
Wuthering Waves. It provides lossless `Engine.ini` editing, documented
presets, before-apply diffs, verified backups, game launching, and Windows
process CPU/priority controls.

The project does not promise performance gains, crash-free operation, or
protection from account action. It does not inject code, access game memory,
hook anti-cheat, install a driver, or bypass the game's technical controls.

## Product Decisions

- Product name: Wuwa ini Tool.
- Initial release: 1.0.0.
- UI languages: Korean and English, selected from the Windows locale and
  overridable in Settings.
- Source identifiers, code comments, contribution documentation, and GitHub
  automation are written in English. User-facing safety and option content is
  bilingual.
- License: MIT, plus a separate bilingual disclaimer.
- Distribution: Windows x64 current-user NSIS setup executable.
- Updates: check automatically after startup; present release notes; download
  and install only after user approval; verify Tauri updater signatures.
- UI icons: `lucide-react` for actions and status. The application mark is an
  original `T` monogram inside configuration brackets. It must not use Kuro
  characters, marks, logos, or recognizable game artwork.

## Architecture

The application is a modular monolith:

- React + TypeScript + Vite render the desktop UI.
- Tauri v2 supplies the desktop shell, tray, single-instance behavior,
  dialogs, updater integration, and restart behavior.
- Rust owns every trust-boundary operation: path validation, game discovery,
  process launch/monitoring, Win32 process control, lossless INI operations,
  backups, restore, and profile persistence.
- Microsoft `windows` bindings provide Win32 process, CPU Set, affinity, and
  priority APIs.
- Versioned JSON files provide the option catalog and built-in presets.
- Versioned JSON in the Tauri application-data directory stores settings,
  custom profiles, and backup metadata. No database is used.

Primary modules:

1. `game_discovery`: detects Kuro Launcher and Steam candidates, validates a
   selected executable, and derives candidate config paths without trusting
   unchecked input.
2. `process_supervisor`: launches the game, watches for
   `Client-Win64-Shipping.exe`, applies process settings, reads them back, and
   emits status events. Closing the window keeps the app in the tray;
   explicit Quit stops monitoring.
3. `cpu_topology`: enumerates CPU Sets, processor groups, logical/physical core
   relationships, and efficiency classes.
4. `ini_document`: preserves unknown sections, keys, comments, order, line
   endings, BOM/encoding, and duplicate-key evidence while applying only
   managed changes.
5. `backup_store`: writes immutable original and timestamped backups with
   metadata and SHA-256 verification.
6. `profile_store`: validates versioned built-in and user profiles and imports
   or exports them without permitting arbitrary file writes.
7. `catalog`: exposes bilingual descriptions, types, constraints, risks,
   source URLs, tested game versions, dates, and support states.
8. `release_update`: checks the signed GitHub Release updater feed without
   blocking normal application startup.

The frontend receives typed DTOs and invokes narrow Tauri commands. It cannot
submit arbitrary process names, arbitrary shell commands, or unrestricted
filesystem paths.

## Data Flows

### Engine.ini apply

1. Revalidate the configured game executable and `Engine.ini` path. The only
   supported target is `Client/Saved/Config/WindowsNoEditor/Engine.ini` in that
   validated game tree; never create or write `UserEngine.ini` or another
   alternate configuration file as a bypass.
2. Refuse writes while the game process is running.
3. Read the file bytes and compare the current hash with the last observed
   hash.
4. Parse without normalizing unrelated content.
5. Merge catalog-managed values and the user's validated custom patch.
6. Render semantic and line-by-line diffs.
7. Require an explicit Apply action.
8. Create and verify a byte-for-byte backup.
9. Write a sibling temporary file, validate it, and atomically replace the
   destination.
10. Read the destination again and verify the expected bytes and hash.

An external hash change invalidates a pending apply. The application reloads
the source and requires a new diff instead of overwriting it.

### Profile sharing

1. Export only versioned profile data: display name, managed INI changes, CPU
   selection, priority, source/provenance label, and creating app version.
2. Exclude absolute paths, backup records, machine identifiers, process IDs,
   and local timestamps that are not required for compatibility.
3. Treat imported files as untrusted input with size, schema, option, range,
   CPU, and priority validation.
4. Show a bilingual import preview and warnings before saving; never apply an
   imported profile automatically.
5. On a name collision, offer a validated new name and preserve the existing
   profile unchanged.

### Game launch and process control

1. Launch the configured official game/launcher path without injection or
   launch-parameter workarounds intended to bypass game controls.
2. Detect the exact game process and validate its executable path.
3. Apply the selected CPU Set preference or hard-affinity mode.
4. Apply the selected Windows priority class.
5. Read both settings back and show success, partial success, access denied,
   unsupported topology, or process exit.

CPU defaults are All cores and Normal. Available priority classes are Idle,
Below Normal, Normal, Above Normal, High, and Realtime. High and Realtime are
never selected automatically and always display a warning icon with hover,
focus, and click tooltips. Realtime remains available by explicit product
decision.

### Update

1. Check the HTTPS GitHub Release `latest.json` endpoint after the UI is ready.
2. If a newer SemVer exists, show the release notes and defer option.
3. Disable installation while config/profile writes are in progress.
4. Download with progress, verify the mandatory updater signature, flush state,
   and start the passive NSIS installer.
5. Treat endpoint errors as non-blocking.

## UI Design

The visual system uses charcoal, warm ivory, and restrained muted gold. It
supports system/light/dark themes and avoids decorative gradients, glow,
dashboard card mosaics, and default component-library styling.

The navigation rail contains Home, Engine.ini, CPU & Priority, Profiles,
Backups, Settings, and About.

- Home shows game detection, process state, active profile, last backup, and a
  primary Launch Game action.
- Engine.ini groups settings by category and shows bilingual explanations,
  support status, risk, tested game version, and source links.
- Advanced Editor is an accordion containing direct text editing. It preserves
  user-owned content and participates in the same diff/backup transaction.
- Diff supports unified and split views and highlights adds, changes, and
  removals. A sticky action bar contains Revert Changes, View Diff, and Backup
  and Apply.
- CPU & Priority visualizes processor groups, P/E or efficiency classes when
  reported, manual selections, all six priority levels, and verified apply
  state.
- Profiles manages immutable built-ins and named custom profiles with clone,
  rename, import, and export.
- Backups provides a timeline, source path, SHA-256, change summary, diff, pin,
  and restore actions.
- Settings owns language, theme, close-to-tray, update preference, and game
  path.
- About presents version, MIT license, disclaimer, source, Issues, and
  contribution links.

Body copy is at least 16px-equivalent, controls/navigation at least
15px-equivalent, secondary metadata at least 14px-equivalent, and ordinary
targets at least 44 CSS px. Keyboard, focus, reduced-motion, high-contrast, and
long Korean/English copy are first-class cases.

## Presets and Option Evidence

Built-in Engine.ini profiles are Vanilla, Balanced, Performance, and Visual
Quality. A catalog option may enter a built-in non-experimental preset only
when its evidence and compatibility state satisfy the repository's promotion
policy.

Catalog states are:

- `verified`: official/upstream semantics plus supported WuWa evidence.
- `community_reported`: a reproducible community claim without sufficient
  independent verification.
- `experimental`: available only through an explicit advanced opt-in.
- `ignored`: the current game appears to ignore or remove the setting.
- `regressed`: a previously useful setting has a known current regression.

The UI distinguishes `present_in_file` from `runtime_verified`. A persisted key
must never be presented as proof that the game used it.

CPU profiles are System Default, Prefer Performance Cores, and Custom. Hard
affinity is an advanced compatibility path. CPU Sets are preferred because they
are group-aware soft affinity and cooperate better with Windows scheduling.

Options such as `MaxCPUCores=0`, `AsyncLoadingThreadPriority=1`, and unverified
TaskGraph/RHI tuning are not shipped as verified CPU controls. Engine CPU
tweaks and Windows process controls remain separate concepts.

## Backup and Restore

- The first observed original is immutable and never automatically removed.
- Every apply and restore creates a backup first.
- Thirty recent automatic backups are retained.
- Pinned backups are never pruned automatically.
- Metadata includes schema version, source path, UTC timestamp, SHA-256,
  original file attributes, application version, and detected game version
  when available.
- Restore previews a diff, verifies the selected backup, preserves the current
  state as a new backup, atomically replaces, and verifies the result.
- User profiles store managed semantic changes and validated custom content,
  not an unchecked command or arbitrary destination path.

## Disclaimer and Legal Boundaries

First run requires acknowledgement of a bilingual notice stating that:

- Wuwa ini Tool is unofficial and unaffiliated with Kuro Games.
- performance improvement, compatibility, and freedom from account action are
  not guaranteed;
- game crashes, setting loss, performance degradation, and account action are
  possible;
- backups should be reviewed and the user remains responsible for use.

The project must not claim that a disclaimer automatically eliminates legal
liability. Release notes and About link to the current Kuro terms. Kuro's
derivative-work guideline excludes software, so the project must not ship the
generated character concept or any recognizable Kuro character derivative.

## Open-source Repository

The repository includes:

- English-first README with a Korean guide;
- MIT `LICENSE` and bilingual `DISCLAIMER.md`;
- `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, and support policy;
- bug, option-evidence, and feature Issue forms;
- a pull request template requiring tests, risk notes, and English comments;
- a catalog-promotion protocol requiring game version, launcher, CPU/GPU/RAM,
  before/after values, FPS and 1% low when relevant, location, test duration,
  artifacts/crashes, and log evidence;
- changelog and release notes.

Comments explain public contracts and non-obvious safety decisions in English;
they do not restate obvious code.

## Testing

Rust tests cover lossless byte round-trips, sections, comments, duplicate keys,
BOM/encoding, CRLF/LF, managed merges, external-change conflicts, backup and
restore hashes, retention, partial failures, profile schema validation, path
validation, CPU topology mapping, and priority conversion.

Windows integration tests use a repository-owned helper process to apply and
read back CPU Sets/affinity/priority without launching the game. Realtime tests
are limited to constant/validation mapping and never change a CI runner to
Realtime.

Frontend tests cover reducer/state behavior, bilingual rendering, option
status, keyboard-accessible accordion and tooltip behavior, diff states,
danger warnings, and update prompts. Browser QA checks desktop and minimum
window sizes, both languages, light/dark, focus, hover, loading, empty, error,
and long-copy behavior.

Release validation requires formatting, linting, typechecking, frontend unit
tests, Rust unit/integration tests, Windows build, NSIS install/uninstall,
tampered-updater rejection, a staging update into 1.0.0, backup preservation,
and a clean Windows VM smoke test.

## CI, Signing, and Release

Pull requests run dependency installation with lockfiles, TypeScript checks,
frontend tests, `cargo fmt --check`, `cargo clippy -- -D warnings`, Rust tests,
security/dependency review, and a Windows Tauri build.

Release automation runs only from protected tags or an explicitly approved
environment. It creates a draft GitHub Release, produces the NSIS setup,
updater signature, `latest.json`, checksums, and SBOM, and publishes only after
verification.

The Tauri updater private key and password live only in protected GitHub
secrets and an offline encrypted recovery location. The public updater key is
committed. Loss of the private key is a release-blocking incident. Authenticode
code signing is separate; a release without an available trusted certificate
must clearly disclose the expected SmartScreen warning.

## External Evidence and Licensing

- Tauri official documentation is authoritative for updater, installer, and
  signing behavior.
- Microsoft documentation is authoritative for Windows CPU and priority APIs.
- Epic documentation establishes upstream Unreal semantics but does not prove
  WuWa runtime support.
- Reddit and DCInside reports are community evidence, not guarantees.
- Arca Live evidence was not accessible during research and is not claimed.
- AlteriaX/WuWa-Configs contains useful observations but no explicit license;
  its text, images, and configuration files are not copied.
- GPL code from other config tools is not copied into this MIT project.

## Out of Scope for 1.0.0

- macOS, Linux, ARM64, Android, and Microsoft Store packages;
- background Windows services that survive explicit app exit;
- game-memory access, injection, hooks, drivers, registry IFEO persistence, or
  anti-cheat/config-monitor bypasses;
- remote preset downloads outside signed application releases;
- automatic performance benchmarking or claims of universal gains.

## Completion Criteria

Version 1.0.0 is ready only when all requirements have traceable test/review
evidence, Windows packaging and signed updater artifacts are verified, backups
survive update/install testing, documentation is complete, and no unresolved
release-blocking risk remains.
