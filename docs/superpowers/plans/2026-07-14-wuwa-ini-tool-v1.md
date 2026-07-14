# Wuwa ini Tool 1.0.0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build, verify, document, and package the complete Wuwa ini Tool 1.0.0 Windows x64 Tauri v2 application.

**Architecture:** A React/TypeScript frontend invokes narrow Tauri commands backed by focused Rust modules. Rust owns all path, file, backup, profile, process, and Win32 trust boundaries; versioned JSON owns the option catalog and built-in presets. Work is organized into four review waves—foundation, configuration safety, Windows process behavior, and release—while this single plan preserves the shared type and release contract.

**Tech Stack:** Tauri v2, Rust, Microsoft `windows`, React, TypeScript, Vite, Vitest, Testing Library, `react-i18next`, `lucide-react`, CSS custom properties, GitHub Actions, NSIS, Tauri updater.

## Global Constraints

- Product/version: `Wuwa ini Tool` `1.0.0`.
- Platform: Windows 10/11 x64; development-only non-Windows stubs may compile but must return `unsupported_platform`.
- License: MIT with a separate Korean/English disclaimer.
- Languages: Korean and English UI; English source identifiers and comments.
- Safety: no injection, game-memory access, anti-cheat hooks, drivers, IFEO persistence, or config-monitor bypass.
- Data: no database; versioned JSON plus byte-for-byte backup files.
- UI: original `T` bracket mark, Lucide icons, charcoal/ivory/muted-gold tokens, 16/15/14px type baselines, and 44px ordinary targets.
- TDD: every behavior starts with a failing test and the expected failure is recorded before implementation. Generated scaffolding and declarative metadata contain no behavior and are verified by build/schema checks.
- Source of truth: `docs/specs/wuwa-ini-tool-v1/01-requirements.md` and `docs/superpowers/specs/2026-07-14-wuwa-ini-tool-design.md`.

---

## Wave 1 — Foundation and UI Contract

### Task 1: Create the Tauri/React testable foundation

**Files:**
- Create: `package.json`, `package-lock.json`, `tsconfig.json`, `tsconfig.node.json`, `vite.config.ts`, `vitest.setup.ts`, `index.html`
- Create: `src/main.tsx`, `src/App.tsx`, `src/App.test.tsx`, `src/vite-env.d.ts`
- Create: `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`
- Create: `src-tauri/src/lib.rs`, `src-tauri/src/main.rs`
- Create: `src-tauri/capabilities/default.json`, `.gitignore`, `.editorconfig`

**Interfaces:**
- Produces: `App`, a Tauri library entrypoint `wuwa_ini_tool_lib::run()`, frontend scripts `test`, `typecheck`, `build`, and Rust library tests.
- Consumes: none.

- [ ] **Step 1: Add the failing shell test**

```tsx
// src/App.test.tsx
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from './App';

describe('App', () => {
  it('renders the product name and version', () => {
    render(<App />);
    expect(screen.getByRole('heading', { name: 'Wuwa ini Tool' })).toBeVisible();
    expect(screen.getByText('1.0.0')).toBeVisible();
  });
});
```

- [ ] **Step 2: Run the test and record RED**

Run: `npm test -- --run src/App.test.tsx`  
Expected: FAIL because `package.json`/`App` does not exist.

- [ ] **Step 3: Add minimal foundation files and dependencies**

Use npm to resolve and lock current compatible releases:

```bash
npm install react react-dom react-i18next i18next lucide-react @tauri-apps/api @tauri-apps/plugin-dialog @tauri-apps/plugin-opener @tauri-apps/plugin-process @tauri-apps/plugin-updater
npm install --save-dev @tauri-apps/cli typescript vite @vitejs/plugin-react vitest jsdom @testing-library/react @testing-library/jest-dom @testing-library/user-event @types/react @types/react-dom eslint prettier
```

`package.json` scripts must be exactly:

```json
{
  "scripts": {
    "dev": "vite",
    "build": "tsc -b && vite build",
    "typecheck": "tsc -b --pretty false",
    "test": "vitest",
    "test:run": "vitest run",
    "tauri": "tauri"
  }
}
```

`src/App.tsx` starts with only the tested contract:

```tsx
export function App() {
  return <main><h1>Wuwa ini Tool</h1><p>1.0.0</p></main>;
}
```

Configure `src-tauri/Cargo.toml` as both `rlib` and `staticlib`, Tauri v2 with tray support, `serde`, `serde_json`, `thiserror`, `sha2`, `tempfile`, `uuid`, `time`, and target-specific `windows`. Set all product versions to `1.0.0` and bundle target to `nsis`.

- [ ] **Step 4: Verify GREEN and both build systems**

Run: `npm test -- --run src/App.test.tsx && npm run typecheck && cargo test --manifest-path src-tauri/Cargo.toml`  
Expected: one frontend test passes, TypeScript exits 0, Rust exits 0.

- [ ] **Step 5: Commit**

```bash
git add package.json package-lock.json tsconfig*.json vite.config.ts vitest.setup.ts index.html src src-tauri .gitignore .editorconfig
git commit -m "chore: scaffold Tauri application"
```

### Task 2: Define shared DTOs, localization, theme, and accessible primitives

**Files:**
- Create: `src/contracts.ts`, `src/contracts.test.ts`
- Create: `src/i18n/index.ts`, `src/i18n/en.json`, `src/i18n/ko.json`, `src/i18n/i18n.test.ts`
- Create: `src/styles/tokens.css`, `src/styles/global.css`
- Create: `src/components/AppIcon.tsx`, `src/components/Button.tsx`, `src/components/Accordion.tsx`, `src/components/Tooltip.tsx`
- Create: `src/components/Accordion.test.tsx`, `src/components/Tooltip.test.tsx`
- Modify: `src/main.tsx`, `src/App.tsx`

**Interfaces:**
- Produces: `AppLanguage = 'en' | 'ko'`, `ThemeMode = 'system' | 'light' | 'dark'`, `CommandError`, `GameStatus`, `PriorityClass`, `CpuSelection`, reusable `Button`, `Accordion`, and `Tooltip`.
- Consumes: foundation from Task 1.

- [ ] **Step 1: Write failing contract and accessibility tests**

```tsx
it('opens the advanced editor accordion from the keyboard', async () => {
  const user = userEvent.setup();
  render(<Accordion title="Advanced editor">editor</Accordion>);
  await user.tab();
  await user.keyboard('{Enter}');
  expect(screen.getByText('editor')).toBeVisible();
});

it('opens a warning tooltip on focus', async () => {
  const user = userEvent.setup();
  render(<Tooltip label="Can make Windows unresponsive"><button>Warning</button></Tooltip>);
  await user.tab();
  expect(screen.getByRole('tooltip')).toHaveTextContent('Can make Windows unresponsive');
});
```

Also assert that every English key exists in Korean and that
`parseCommandError` rejects unknown error shapes.

- [ ] **Step 2: Run RED**

Run: `npm test -- --run src/contracts.test.ts src/i18n/i18n.test.ts src/components/Accordion.test.tsx src/components/Tooltip.test.tsx`  
Expected: FAIL because contracts and components do not exist.

- [ ] **Step 3: Implement minimal shared contracts and primitives**

Use discriminated unions:

```ts
export type PriorityClass = 'idle' | 'belowNormal' | 'normal' | 'aboveNormal' | 'high' | 'realtime';
export type CpuSelection =
  | { mode: 'all' }
  | { mode: 'performanceCores' }
  | { mode: 'cpuSets'; ids: number[] }
  | { mode: 'hardAffinity'; group: number; mask: string };
export type CommandError = { code: string; message: string; details?: Record<string, unknown> };
```

`Tooltip` must open on hover, focus, and click; close on Escape; connect trigger and content with ARIA. Tokens define `--color-bg`, `--color-panel`, `--color-text`, `--color-muted`, `--color-accent`, `--color-danger`, focus ring, spacing, radii, and the 16/15/14/44px baselines.

- [ ] **Step 4: Run GREEN and accessibility-focused tests**

Run: `npm test -- --run src/contracts.test.ts src/i18n/i18n.test.ts src/components/Accordion.test.tsx src/components/Tooltip.test.tsx && npm run typecheck`  
Expected: all named tests pass and TypeScript exits 0.

- [ ] **Step 5: Commit**

```bash
git add src
git commit -m "feat: add localized design foundation"
```

---

## Wave 2 — Lossless Configuration, Evidence, and Recovery

### Task 3: Implement the lossless INI document model

**Files:**
- Create: `src-tauri/tests/ini_document.rs`
- Create: `src-tauri/src/ini_document/mod.rs`, `encoding.rs`, `line.rs`, `merge.rs`, `error.rs`
- Modify: `src-tauri/src/lib.rs`
- Create: `src-tauri/tests/fixtures/utf8-crlf.ini`, `utf8-bom-lf.ini`, `utf16le-crlf.ini`

**Interfaces:**
- Produces: `IniDocument::parse(&[u8]) -> Result<IniDocument, IniError>`, `IniDocument::merge(&[ManagedChange]) -> Result<MergePreview, IniError>`, `MergePreview { before: Vec<u8>, after: Vec<u8>, semantic_changes: Vec<SemanticChange> }`.
- Consumes: no application module.

- [ ] **Step 1: Write failing byte-preservation tests**

```rust
#[test]
fn untouched_document_round_trips_byte_for_byte() {
    let bytes = include_bytes!("fixtures/utf8-crlf.ini");
    let document = IniDocument::parse(bytes).unwrap();
    assert_eq!(document.as_bytes(), bytes);
}

#[test]
fn merge_changes_only_the_managed_key() {
    let bytes = b"; keep\r\n[SystemSettings]\r\nr.Foo=1\r\ncustom=stay\r\n";
    let preview = IniDocument::parse(bytes).unwrap().merge(&[
        ManagedChange::set("SystemSettings", "r.Foo", "2")
    ]).unwrap();
    assert_eq!(preview.after, b"; keep\r\n[SystemSettings]\r\nr.Foo=2\r\ncustom=stay\r\n");
}
```

Add cases for UTF-8 BOM, UTF-16LE BOM, LF/CRLF, comments, repeated sections,
duplicate managed keys (return `IniError::AmbiguousManagedKey`), deletion, and
insertion into a missing section.

- [ ] **Step 2: Run RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test ini_document`  
Expected: FAIL because `ini_document` is missing.

- [ ] **Step 3: Implement a line-preserving parser and merger**

Represent each decoded line as the original text plus its exact terminator.
Detect only UTF-8, UTF-8 BOM, and UTF-16LE BOM; return
`IniError::UnsupportedEncoding` for invalid/unsupported bytes. Never reorder or
normalize untouched lines. A managed key match is case-insensitive after ASCII
trim around section/key names, but the original spelling is preserved.

- [ ] **Step 4: Run GREEN, then full Rust tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test ini_document && cargo test --manifest-path src-tauri/Cargo.toml`  
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/ini_document src-tauri/tests src-tauri/src/lib.rs
git commit -m "feat: add lossless INI merge engine"
```

### Task 4: Add verified backup, atomic apply, restore, and retention

**Files:**
- Create: `src-tauri/tests/backup_store.rs`
- Create: `src-tauri/src/backup_store/mod.rs`, `model.rs`, `atomic.rs`, `retention.rs`, `error.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Produces: `BackupStore::create`, `BackupStore::apply`, `BackupStore::restore`, `BackupStore::list`, `BackupStore::pin`; `BackupRecord { id, source_path, created_at, sha256, reason, pinned, original_attributes }`.
- Consumes: `MergePreview` from Task 3.

- [ ] **Step 1: Write failing transactional tests**

```rust
#[test]
fn apply_creates_verified_backup_before_replace() {
    let fixture = TestStore::new(b"before");
    let result = fixture.store.apply(fixture.source(), b"after", ApplyReason::Preset).unwrap();
    assert_eq!(std::fs::read(fixture.source()).unwrap(), b"after");
    assert_eq!(std::fs::read(result.backup_path).unwrap(), b"before");
    assert_eq!(result.backup.sha256, sha256_hex(b"before"));
}
```

Add tests for immutable first-original, backup-before-restore, source hash
conflict, failed temp write leaving source unchanged, readback mismatch, 30
unpinned retention, pinned retention, and path traversal rejection.

- [ ] **Step 2: Run RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test backup_store`  
Expected: FAIL because `backup_store` is missing.

- [ ] **Step 3: Implement the backup transaction**

Write the backup to `<app-data>/backups/<source-id>/<timestamp>-<uuid>.ini`,
fsync it, verify SHA-256, write the destination sibling temp file, fsync,
replace atomically, read back, and verify. Persist metadata as versioned JSON
through the same temp-and-replace primitive. On Windows use a replacement API
that can replace an existing destination; on test hosts use same-filesystem
rename with rollback-safe ordering.

- [ ] **Step 4: Run GREEN and failure tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test backup_store && cargo test --manifest-path src-tauri/Cargo.toml`  
Expected: all tests pass without leftover temp files.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/backup_store src-tauri/tests src-tauri/src/lib.rs
git commit -m "feat: add verified config recovery"
```

### Task 5: Add option catalog, built-in profiles, and custom profiles

**Files:**
- Create: `catalog/options.json`, `catalog/presets.json`, `catalog/schema/*.schema.json`
- Create: `src-tauri/tests/catalog_profiles.rs`
- Create: `src-tauri/src/catalog/mod.rs`, `model.rs`, `validation.rs`
- Create: `src-tauri/src/profile_store/mod.rs`, `model.rs`, `error.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Produces: `Catalog::load_embedded()`, `ProfileStore::{list,get,save,rename,clone_profile,export,import}`, `OptionStatus`, `ProfilePatch`.
- Consumes: `ManagedChange` and shared CPU/profile enums.

- [ ] **Step 1: Write failing schema and safety tests**

```rust
#[test]
fn community_reported_option_cannot_enter_verified_builtin_preset() {
    let catalog = fixture_catalog(OptionStatus::CommunityReported);
    let preset = fixture_builtin_preset("performance", "r.Unverified");
    assert_eq!(validate_builtin(&catalog, &preset), Err(CatalogError::UnverifiedPresetOption("r.Unverified".into())));
}
```

Add cases for bilingual text presence, source URL, tested version/date,
range/type validation, schema-version rejection, safe filename export, unknown
profile key rejection, and CPU/priority round-trip.

- [ ] **Step 2: Run RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test catalog_profiles`  
Expected: FAIL because modules and JSON do not exist.

- [ ] **Step 3: Implement catalog/profile validation and conservative data**

Ship complete Vanilla and CPU profiles. Balanced, Performance, and Visual
Quality exist as valid named built-ins but contain only entries whose evidence
meets `verified`; if no such non-default entry is independently supported, the
preset remains intentionally conservative and its bilingual description says
so. Never copy third-party descriptions or configs.

- [ ] **Step 4: Run GREEN and JSON schema checks**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test catalog_profiles && cargo test --manifest-path src-tauri/Cargo.toml`  
Expected: all tests pass and embedded JSON validates.

- [ ] **Step 5: Commit**

```bash
git add catalog src-tauri/src/catalog src-tauri/src/profile_store src-tauri/tests src-tauri/src/lib.rs
git commit -m "feat: add evidence-aware profiles"
```

---

## Wave 3 — Game Discovery, Windows Scheduling, and Tauri Commands

### Task 6: Implement validated game discovery and process identity

**Files:**
- Create: `src-tauri/tests/game_discovery.rs`
- Create: `src-tauri/src/game_discovery/mod.rs`, `paths.rs`, `windows_registry.rs`, `error.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Produces: `GameInstallation { channel, root, executable, engine_ini }`, `discover_installations()`, `validate_game_executable(path)`, `derive_engine_ini(path)`.
- Consumes: standard filesystem and target-specific registry access.

- [ ] **Step 1: Write failing path tests**

```rust
#[test]
fn derives_kuro_saved_config_from_validated_client_executable() {
    let installation = fixture_kuro_tree();
    let result = validate_game_executable(installation.client_exe()).unwrap();
    assert_eq!(result.engine_ini, installation.root().join("Wuthering Waves Game/Client/Saved/Config/WindowsNoEditor/Engine.ini"));
}
```

Add Steam layout, wrong filename, missing executable, non-file, symlink escape,
case-insensitive Windows filename, and manual selection cases.

- [ ] **Step 2: Run RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test game_discovery`  
Expected: FAIL because the module is missing.

- [ ] **Step 3: Implement discovery with validated candidates**

Check documented registry/library candidates on Windows, but accept only a
candidate whose executable and derived Client directory exist. Manual selection
must pass the same validator. Store canonical paths and compare process images
against the configured executable directory before applying settings.

- [ ] **Step 4: Run GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test game_discovery && cargo test --manifest-path src-tauri/Cargo.toml`  
Expected: all tests pass; non-Windows discovery returns an empty candidate list.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/game_discovery src-tauri/tests src-tauri/src/lib.rs
git commit -m "feat: add safe game discovery"
```

### Task 7: Implement CPU topology, affinity, priority, and readback

**Files:**
- Create: `src-tauri/tests/process_control.rs`
- Create: `src-tauri/src/process_control/mod.rs`, `model.rs`, `validation.rs`, `unsupported.rs`, `windows.rs`, `error.rs`
- Create: `src-tauri/src/bin/process_fixture.rs`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml`

**Interfaces:**
- Produces: `ProcessController::{topology,apply,readback}`, `PriorityClass`, `CpuSelection`, `ApplyReport`, and a Windows-only fixture process.
- Consumes: validated PID/path from Task 6; Microsoft `windows` APIs.

- [ ] **Step 1: Write failing pure validation tests**

```rust
#[test]
fn every_priority_round_trips_without_changing_the_default() {
    for value in PriorityClass::ALL {
        assert_eq!(PriorityClass::from_wire(value.as_wire()).unwrap(), value);
    }
    assert_eq!(PriorityClass::default(), PriorityClass::Normal);
}

#[test]
fn hard_affinity_rejects_bits_outside_the_selected_group() {
    let topology = fixture_topology_with_group_mask(0, 0b1111);
    assert_eq!(validate_selection(&topology, &CpuSelection::HardAffinity { group: 0, mask: 0b1_0000 }), Err(ProcessError::InvalidAffinityMask));
}
```

Add tests for performance-core selection from relative `EfficiencyClass`, empty
CPU Sets, multiple groups, all six priority mappings, access-denied reporting,
and unsupported platform.

- [ ] **Step 2: Run RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test process_control`  
Expected: FAIL because process control is missing.

- [ ] **Step 3: Implement pure model/validation, then Windows backend**

Use `GetSystemCpuSetInformation`, `SetProcessDefaultCpuSets`, processor-group
APIs, `SetProcessAffinityMask` only for validated single-group hard affinity,
`SetPriorityClass`, and matching get/readback APIs. Open the process with the
minimum query/set-information rights. Never elevate automatically. Map Win32
errors into stable codes without leaking raw paths in user-facing messages.

- [ ] **Step 4: Run GREEN locally and Windows integration conditionally**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test process_control`  
Expected on macOS: pure tests pass and backend reports unsupported.  
Run on Windows: `cargo test --manifest-path src-tauri/Cargo.toml --test process_control -- --include-ignored`  
Expected: helper process accepts and reads back safe classes/CPU selections;
Realtime is never applied to the CI runner.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/process_control src-tauri/src/bin/process_fixture.rs src-tauri/tests src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "feat: add Windows process tuning"
```

### Task 8: Add the supervisor, tray lifecycle, and narrow command API

**Files:**
- Create: `src-tauri/tests/supervisor.rs`
- Create: `src-tauri/src/supervisor/mod.rs`, `state.rs`, `events.rs`
- Create: `src-tauri/src/commands/mod.rs`, `config.rs`, `game.rs`, `profiles.rs`, `process.rs`, `backups.rs`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/main.rs`, `src-tauri/capabilities/default.json`

**Interfaces:**
- Produces typed commands `get_app_snapshot`, `preview_ini`, `apply_ini`, `restore_backup`, `save_profile`, `discover_game`, `launch_game`, `get_cpu_topology`, `apply_process_settings`, and supervisor status events.
- Consumes: Tasks 3–7.

- [ ] **Step 1: Write failing supervisor and command-validation tests**

```rust
#[test]
fn supervisor_applies_once_to_the_validated_game_process() {
    let backend = FakeBackend::with_process(validated_process(42));
    let mut supervisor = Supervisor::new(backend, safe_profile());
    supervisor.tick().unwrap();
    supervisor.tick().unwrap();
    assert_eq!(supervisor.backend().apply_calls(), vec![42]);
}
```

Add wrong-path same-name process, process restart, partial apply, exit, game-
running write rejection, stale preview token, arbitrary-path rejection, and
close-to-tray/explicit-quit state tests.

- [ ] **Step 2: Run RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test supervisor`  
Expected: FAIL because supervisor/commands are missing.

- [ ] **Step 3: Implement state machine and least-privilege capabilities**

The supervisor states are `Idle`, `Launching`, `WaitingForGame`, `Applying`,
`Active`, `Partial`, `Denied`, and `Exited`. Poll with bounded backoff while the
tray app is alive. Capabilities permit only updater check/install, process
restart, dialog selection, and opener links required by the UI; filesystem and
shell plugins are not granted broad frontend access.

- [ ] **Step 4: Run GREEN and full Rust suite**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test supervisor && cargo test --manifest-path src-tauri/Cargo.toml`  
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src-tauri
git commit -m "feat: integrate secure Tauri commands"
```

### Task 9: Build the complete application surfaces

**Files:**
- Create: `src/api/commands.ts`, `src/api/commands.test.ts`
- Create: `src/state/app-state.tsx`, `src/state/app-state.test.tsx`
- Create: `src/layout/AppShell.tsx`, `src/layout/Navigation.tsx`
- Create: `src/pages/HomePage.tsx`, `EngineIniPage.tsx`, `CpuPriorityPage.tsx`, `ProfilesPage.tsx`, `BackupsPage.tsx`, `SettingsPage.tsx`, `AboutPage.tsx`
- Create: `src/features/ini/OptionEditor.tsx`, `AdvancedEditor.tsx`, `DiffViewer.tsx`, `ApplyBar.tsx`
- Create: `src/features/process/CpuTopology.tsx`, `PriorityPicker.tsx`, `ProcessStatus.tsx`
- Create: `src/features/profiles/ProfileList.tsx`
- Create: `src/features/backups/BackupTimeline.tsx`
- Create: corresponding `*.test.tsx` files for each feature
- Modify: `src/App.tsx`, `src/styles/global.css`, localization JSON

**Interfaces:**
- Produces: all user-visible routes/surfaces and a single typed command adapter.
- Consumes: Task 2 contracts and Task 8 commands/events.

- [ ] **Step 1: Write failing workflow tests**

```tsx
it('previews a diff before applying a preset', async () => {
  const user = userEvent.setup();
  const api = fakeApi({ preview: diffFixture() });
  render(<TestApp api={api} initialPage="engineIni" />);
  await user.click(screen.getByRole('button', { name: /performance/i }));
  await user.click(screen.getByRole('button', { name: /view diff/i }));
  expect(screen.getByText('+r.Example=1')).toBeVisible();
  expect(api.applyIni).not.toHaveBeenCalled();
});

it.each(['high', 'realtime'])('shows an accessible warning for %s', async (priority) => {
  render(<PriorityPicker value={priority as PriorityClass} onChange={() => {}} />);
  expect(screen.getByRole('button', { name: new RegExp(`${priority} warning`, 'i') })).toHaveAccessibleDescription();
});
```

Add tests for first-run acknowledgement, external-change conflict, game-running
write lock, raw-editor accordion, custom profile save/import error, restore
preview, bilingual switch, theme, close-to-tray copy, loading/empty/error
states, updater prompt, and keyboard navigation.

- [ ] **Step 2: Run RED**

Run: `npm test -- --run`  
Expected: new workflow tests fail because pages/features are missing.

- [ ] **Step 3: Implement the smallest complete UI against the typed adapter**

Use a route-less local page state because this is one desktop window. Keep
command calls only in `src/api/commands.ts`; keep pending preview tokens in app
state; disable Apply until a current preview exists. Use individual Lucide
imports. Render High/Realtime tooltips on hover, focus, and click. Use original
copy, never copied third-party option descriptions.

- [ ] **Step 4: Run GREEN, typecheck, and production build**

Run: `npm test -- --run && npm run typecheck && npm run build`  
Expected: all tests pass, TypeScript exits 0, Vite build exits 0.

- [ ] **Step 5: Commit**

```bash
git add src
git commit -m "feat: build configuration desktop UI"
```

---

## Wave 4 — Branding, Update, Repository, and Release Gates

### Task 10: Create deterministic original branding and Windows icons

**Files:**
- Create: `assets/brand/wuwa-ini-tool-mark.svg`
- Create: `src-tauri/icons/icon.png`, `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.ico`
- Modify: `src/components/AppIcon.tsx`, `src-tauri/tauri.conf.json`
- Create: `scripts/verify-icons.mjs`

**Interfaces:**
- Produces: an original bracketed `T` vector source and Tauri icon set.
- Consumes: design tokens from Task 2.

- [ ] **Step 1: Add a failing deterministic icon verifier**

```js
const required = ['32x32.png', '128x128.png', '128x128@2x.png', 'icon.ico'];
for (const file of required) {
  if (!existsSync(resolve('src-tauri/icons', file))) throw new Error(`missing ${file}`);
}
if (readFileSync('assets/brand/wuwa-ini-tool-mark.svg', 'utf8').match(/kuro|character|image href/i)) {
  throw new Error('brand source must be original vector geometry');
}
```

- [ ] **Step 2: Run RED**

Run: `node scripts/verify-icons.mjs`  
Expected: FAIL because icon files are missing.

- [ ] **Step 3: Implement the mark as simple SVG geometry and generate icons**

Recreate the approved concept with a charcoal rounded square, ivory uppercase
`T`, and muted-gold square brackets using only paths/rectangles. Do not trace
the generated raster and do not include game assets. Generate the icon set with
`npm run tauri icon assets/brand/wuwa-ini-tool-mark.svg`.

- [ ] **Step 4: Run GREEN and inspect small sizes**

Run: `node scripts/verify-icons.mjs`  
Expected: exits 0. Inspect 16x16/32x32 rendering for a readable T and brackets.

- [ ] **Step 5: Commit**

```bash
git add assets src-tauri/icons src-tauri/tauri.conf.json src/components/AppIcon.tsx scripts/verify-icons.mjs
git commit -m "feat: add original application branding"
```

### Task 11: Add signed update UX and release configuration

**Files:**
- Create: `src/features/update/UpdatePrompt.tsx`, `UpdatePrompt.test.tsx`, `src/features/update/update-service.ts`, `update-service.test.ts`
- Modify: `src/App.tsx`, `src-tauri/src/lib.rs`, `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`

**Interfaces:**
- Produces: `UpdateService.check()`, explicit `downloadAndInstall`, progress state, defer state, and safe restart.
- Consumes: Tauri updater/process plugins and write-in-progress state.

- [ ] **Step 1: Write failing updater UX tests**

```tsx
it('never downloads until the user approves the discovered version', async () => {
  const updater = fakeUpdater({ version: '1.0.1', notes: 'Fixes' });
  render(<UpdatePrompt updater={updater} writeInProgress={false} />);
  await screen.findByText('1.0.1');
  expect(updater.downloadAndInstall).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: /update now/i }));
  expect(updater.downloadAndInstall).toHaveBeenCalledOnce();
});
```

Add endpoint failure non-blocking, write-in-progress disabled, progress, defer,
and release-note rendering tests.

- [ ] **Step 2: Run RED**

Run: `npm test -- --run src/features/update`  
Expected: FAIL because update feature is missing.

- [ ] **Step 3: Implement updater with production HTTPS endpoint and public key**

Use `https://github.com/INIRU/Wuwa-ini-Tool/releases/latest/download/latest.json`,
passive Windows install mode, `createUpdaterArtifacts: true`, and the committed
public updater key. Keep only check, download/install, and restart permissions.

- [ ] **Step 4: Run GREEN and config validation**

Run: `npm test -- --run src/features/update && npm run build && npm run tauri info`  
Expected: tests/build pass; Tauri reports valid updater configuration.

- [ ] **Step 5: Commit**

```bash
git add src src-tauri
git commit -m "feat: add signed update workflow"
```

### Task 12: Add open-source governance, CI, and release automation

**Files:**
- Create: `README.md`, `README.ko.md`, `LICENSE`, `DISCLAIMER.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, `SUPPORT.md`, `CHANGELOG.md`
- Create: `.github/ISSUE_TEMPLATE/bug.yml`, `option-evidence.yml`, `feature.yml`, `config.yml`
- Create: `.github/pull_request_template.md`, `.github/dependabot.yml`
- Create: `.github/workflows/ci.yml`, `.github/workflows/release.yml`
- Create: `scripts/check-version.mjs`, `scripts/validate-catalog.mjs`

**Interfaces:**
- Produces: contributor contract, evidence intake, protected PR validation, and draft release automation.
- Consumes: all build/test scripts, catalog schema, and Tauri bundle.

- [ ] **Step 1: Add failing repository-policy checks**

```js
const required = ['LICENSE', 'DISCLAIMER.md', 'CONTRIBUTING.md', 'SECURITY.md', 'README.ko.md'];
for (const path of required) if (!existsSync(path)) throw new Error(`missing ${path}`);
const versions = [pkg.version, tauri.version, cargo.package.version];
if (new Set(versions).size !== 1 || versions[0] !== '1.0.0') throw new Error(`version mismatch: ${versions}`);
```

- [ ] **Step 2: Run RED**

Run: `node scripts/check-version.mjs && node scripts/validate-catalog.mjs`  
Expected: FAIL because repository documents/workflows/scripts are incomplete.

- [ ] **Step 3: Implement governance and workflows**

`ci.yml` runs npm clean install, policy scripts, frontend tests/typecheck/build,
Rust fmt/clippy/tests, dependency review, and Windows Tauri build. `release.yml`
runs only on `v*` tags or approved manual dispatch, requires version/tag match,
uses protected updater secrets, creates a draft release, uploads NSIS, `.sig`,
`latest.json`, checksums, and SBOM, and never uses `pull_request_target` secrets.

Issue evidence form requires game version, launcher, CPU/GPU/RAM, exact option,
before/after value, FPS/1% low when claimed, test area/duration, artifacts,
crashes, and logs. Security reporting uses private GitHub advisories.

- [ ] **Step 4: Run GREEN and workflow syntax review**

Run: `node scripts/check-version.mjs && node scripts/validate-catalog.mjs && npm test -- --run && cargo fmt --manifest-path src-tauri/Cargo.toml -- --check && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`  
Expected: every command exits 0.

- [ ] **Step 5: Commit**

```bash
git add README.md README.ko.md LICENSE DISCLAIMER.md CONTRIBUTING.md CODE_OF_CONDUCT.md SECURITY.md SUPPORT.md CHANGELOG.md .github scripts
git commit -m "docs: prepare open source release"
```

### Task 13: Integrate, verify Windows release, and close traceability

**Files:**
- Modify: `docs/specs/wuwa-ini-tool-v1/04-test.md`, `checkpoint.json`, `evidence.jsonl`, `risk-ledger.jsonl`
- Modify only as proven necessary: files from Tasks 1–12

**Interfaces:**
- Produces: complete verification evidence, GitHub repository, draft/verified 1.0.0 release, and final handoff.
- Consumes: every prior task.

- [ ] **Step 1: Run the complete local gate**

```bash
npm ci
node scripts/check-version.mjs
node scripts/validate-catalog.mjs
node scripts/verify-icons.mjs
npm test -- --run
npm run typecheck
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
git diff --check
```

Expected: all commands exit 0 with no test failures or warnings treated as
errors.

- [ ] **Step 2: Push and require Windows CI**

Create or use `INIRU/Wuwa-ini-Tool`, push `main`, and require the Windows CI
job to build the Tauri application and run ignored Windows process integration
tests. Record the workflow URL and commit SHA.

- [ ] **Step 3: Configure updater signing and verify artifacts**

Generate a password-protected Tauri updater key if no recoverable project key
exists, place only the public key in source, store the private key/password in
protected GitHub secrets and an encrypted offline recovery location, then run a
draft `v1.0.0` release. Verify NSIS setup, `.sig`, `latest.json`, SHA-256 sums,
and SBOM. Do not print secret material.

- [ ] **Step 4: Perform Windows release checks**

On a clean Windows environment: install current-user NSIS; open both languages
and themes; validate Kuro/Steam/manual path errors; run the helper-process CPU
integration; confirm High/Realtime warnings without applying Realtime in test;
exercise INI apply, external conflict, backup, restore, and update preservation;
verify a modified installer is rejected; uninstall. Record exact evidence.

- [ ] **Step 5: Close requirement traceability and risks**

Map every `REQ-*` row in `04-test.md` to a command/test/review/evidence ID. Keep
any unverified Windows, Authenticode, Kuro-policy, or updater-recovery item open
and do not publish the draft while a release blocker remains. Update project
docs for stable behavior.

- [ ] **Step 6: Commit evidence and publish only when ready**

```bash
git add docs/specs/wuwa-ini-tool-v1
git commit -m "docs: record 1.0.0 verification"
git push origin main
```

Publish `v1.0.0` only after the draft assets and clean-Windows gate pass. Then
verify the public `latest.json` endpoint unauthenticated and record the final
release URL.

## Requirement Coverage Review

| Requirements | Implementing tasks |
| --- | --- |
| REQ-PROD-001..004 | 1, 2, 10, 12 |
| REQ-INI-001..006 | 3, 4, 6, 8, 9 |
| REQ-CAT-001..004 | 5, 9, 12 |
| REQ-CPU-001..006 | 6, 7, 8, 9 |
| REQ-BACKUP-001..003 | 4, 8, 9 |
| REQ-UI-001..004 | 2, 9, 10 |
| REQ-UPD-001..003 | 11, 12, 13 |
| REQ-OSS-001..002 | 5, 12, 13 |
| REQ-SAFE-001..002 | 9, 12, 13 |

Self-review result: every requirement is assigned to at least one behavior
task and one final verification path. Public interfaces use the same
`PriorityClass`, `CpuSelection`, `ManagedChange`, `MergePreview`, and
`BackupRecord` names throughout the plan. No unfinished placeholder is part of
the implementation contract.
