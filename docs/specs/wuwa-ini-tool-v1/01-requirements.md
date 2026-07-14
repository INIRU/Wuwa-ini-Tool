# Wuwa ini Tool 1.0.0 Requirements

Status: Approved  
Source design: `docs/superpowers/specs/2026-07-14-wuwa-ini-tool-design.md`

## Product

- REQ-PROD-001: Ship a Windows 10/11 x64 Tauri v2 application named Wuwa ini
  Tool at version 1.0.0 under the MIT license.
- REQ-PROD-002: Provide Korean and English UI with automatic locale selection
  and an in-app override.
- REQ-PROD-003: Use English for source identifiers, code comments, GitHub
  automation, and contributor-facing code documentation.
- REQ-PROD-004: Use an original `T` configuration-bracket mark and no Kuro
  character, logo, or recognizable official art.

## Engine.ini

- REQ-INI-001: Detect Kuro Launcher and Steam installations and support a
  validated manual executable selection.
- REQ-INI-002: Preserve unrelated sections, keys, comments, ordering, duplicate
  evidence, line endings, file attributes, and supported encodings.
- REQ-INI-003: Show semantic and line diffs before every apply or restore.
- REQ-INI-004: Block writes while the game is running and invalidate pending
  writes after an external file change.
- REQ-INI-005: Back up, atomically replace, read back, and hash-verify every
  apply or restore.
- REQ-INI-006: Provide a keyboard-accessible accordion raw editor whose changes
  use the same diff and backup transaction.
- REQ-INI-007: Edit only the actual `Engine.ini` derived from the validated
  game's `Client` tree; never create or write `UserEngine.ini` or another
  alternate configuration file as a bypass.

## Catalog and Profiles

- REQ-CAT-001: Store bilingual description, type, constraints, risk, status,
  tested version/date/hardware, and source URL for catalog options.
- REQ-CAT-002: Distinguish verified, community reported, experimental, ignored,
  and regressed options and distinguish file presence from runtime verification.
- REQ-CAT-003: Ship Vanilla, Balanced, Performance, and Visual Quality built-in
  profiles without promoting unsupported claims as verified.
- REQ-CAT-004: Save, load, clone, rename, import, and export validated custom
  profiles including managed INI and CPU settings.
- REQ-CAT-005: Use a portable versioned profile-sharing format that excludes
  local paths, backups, and device identifiers; validate untrusted imports,
  preview them before saving or applying, and resolve name collisions without
  overwriting an existing profile.
- REQ-CAT-006: Allow explicit custom INI section/key/value entries that are not
  in the catalog, label them as user-defined and runtime-unverified, validate
  their INI syntax, and preserve them through profile save/export/import and
  the normal diff/backup transaction.

## CPU and Game Process

- REQ-CPU-001: Launch the configured game and monitor the validated
  `Client-Win64-Shipping.exe` process while the app or tray process is active.
- REQ-CPU-002: Enumerate processor groups, CPU Sets, physical/logical topology,
  and efficiency classes when Windows reports them.
- REQ-CPU-003: Support all cores, performance-core preference, manual CPU Sets,
  and advanced hard affinity with processor-group-aware validation.
- REQ-CPU-004: Support Idle, Below Normal, Normal, Above Normal, High, and
  Realtime priority classes; default to Normal; never auto-select High or
  Realtime; display accessible warnings for High and Realtime.
- REQ-CPU-005: Read settings back and report success, partial success, access
  denied, unsupported topology, and process exit.
- REQ-CPU-006: Do not inject, read/write game memory, hook anti-cheat, install a
  driver, use IFEO persistence, or bypass game technical controls.

## Backup and Recovery

- REQ-BACKUP-001: Keep the first original backup indefinitely and create a
  backup before every apply and restore.
- REQ-BACKUP-002: Retain 30 recent automatic backups and never automatically
  prune pinned backups.
- REQ-BACKUP-003: Store versioned metadata and SHA-256 and verify backup and
  restore integrity.

## UI and Accessibility

- REQ-UI-001: Provide Home, Engine.ini, CPU & Priority, Profiles, Backups,
  Settings, and About surfaces with a charcoal/ivory/muted-gold visual system.
- REQ-UI-002: Support system, light, and dark themes; keyboard and visible focus;
  reduced motion; accessible warnings; and long Korean/English content.
- REQ-UI-003: Meet 16px-equivalent body, 15px-equivalent control, 14px-equivalent
  secondary metadata, and 44 CSS px ordinary target baselines.
- REQ-UI-004: Use Lucide icons by individual import and shared accessible UI
  primitives instead of an unmodified component-library appearance.

## Updates, GitHub, and Release

- REQ-UPD-001: Check signed GitHub Releases automatically after startup and ask
  before download/install.
- REQ-UPD-002: Keep updater failures non-blocking and prevent install while
  config/profile writes are active.
- REQ-UPD-003: Build a current-user Windows x64 NSIS setup and signed Tauri
  updater artifacts through protected GitHub automation.
- REQ-OSS-001: Include MIT license, bilingual disclaimer, contribution/security/
  conduct/support docs, Issue forms, PR template, changelog, and release notes.
- REQ-OSS-002: Require reproducible evidence before catalog promotion and never
  copy unlicensed or GPL-incompatible reference content.

## Safety and Disclaimer

- REQ-SAFE-001: Require first-run acknowledgement that the app is unofficial,
  performance and account safety are not guaranteed, and crashes, setting loss,
  degradation, or account action are possible.
- REQ-SAFE-002: Never represent the disclaimer as automatically eliminating all
  legal liability.
