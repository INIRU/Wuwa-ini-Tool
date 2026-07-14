import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type PriorityClass = "idle" | "below_normal" | "normal" | "above_normal" | "high" | "realtime";
export type CpuSelection =
  | { mode: "all" }
  | { mode: "prefer_performance" }
  | { mode: "manual_cpu_sets"; ids: number[] }
  | { mode: "hard_affinity"; group: number; mask: string };
export type SupervisorState = "idle" | "launching" | "waiting_for_game" | "applying" | "active" | "partial" | "denied" | "exited";
export type FocusRuntimeAvailability = "available" | "topology_unavailable" | "telemetry_unavailable" | "unavailable";
export type FocusProcessStatus = "eligible" | "access_denied" | "different_user" | "different_session" | "session_zero" | "system" | "protected" | "critical" | "game" | "tool" | "launcher_or_overlay" | "foreground" | "visible_window" | "active_audio" | "pinned" | "communication" | "recording" | "streaming" | "capture_overlay" | "priority_not_normal";

export interface GameInstallation { channel: "kuro" | "steam" | "manual"; requires_user_confirmation: boolean; game_root: string; executable: string; engine_ini: string }
export interface ObservedGame { pid: number; creation_time_100ns: string; canonical_image: string }
export interface AppSnapshot { version: string; installation: GameInstallation | null; supervisor_state: SupervisorState; active_process: ObservedGame | null; focus_runtime_availability: FocusRuntimeAvailability | null; focus_pinned_executables: string[] }
export interface GameCandidate { token: string; installation: GameInstallation }
export interface ManagedChange { section: string; key: string; value: string | null }
export type IniPreviewRequest = { kind: "paste"; text: string } | { kind: "managed"; changes: ManagedChange[] };
export interface DiffLine { kind: "context" | "removed" | "added" | "metadata"; old_line: number | null; new_line: number | null; text: string }
export interface SemanticChange { section: string; key: string; before: string | null; after: string | null }
export interface IniPreview { token: string; before_bytes: number; after_bytes: number; candidate_text: string; diff: DiffLine[]; semantic_changes: SemanticChange[]; before_encoding: string; after_encoding: string; before_line_endings: string; after_line_endings: string; byte_only_change: boolean }
export interface ProcessSettingsRequest { cpu_selection: CpuSelection; priority: PriorityClass; dangerous_priority_acknowledged: boolean }
export interface CpuSetInfo { id: number; group: number; logical_processor_index: number; core_index: number; last_level_cache_index: number; numa_node_index: number; efficiency_class: number; parked: boolean; allocated: boolean; allocated_to_target: boolean; realtime: boolean }
export interface CpuTopology { cpu_sets: CpuSetInfo[]; groups: { group: number; active_mask: string }[] }
export interface ApplyReport { status: "success" | "partial" | "denied" | "unsupported" | "exited"; cpu: { requested: CpuSelection; applied: CpuSelection | null; error_code: string | null }; priority: { requested: PriorityClass; applied: PriorityClass | null; error_code: string | null } }
export interface FocusProcessIdentity { pid: number; creation_time_100ns: string; canonical_image: string }
export interface FocusCandidate { identity: FocusProcessIdentity; display_name: string; current_priority: PriorityClass; status: FocusProcessStatus }
export interface FocusThresholds { sample_interval_ms: number; aggregate_contention_basis_points: number; release_basis_points: number; game_hot_thread_basis_points: number; competitor_basis_points: number; sustained_samples: number; release_samples: number }
export interface FocusPreviewEnvelope { token: string; thresholds: FocusThresholds; candidates: FocusCandidate[]; runtime_availability: FocusRuntimeAvailability }
export interface FocusLifecycleReport { epoch: number; process: ObservedGame | null; status: "no_changes" | "recovered" | "activated" | "armed" | "applied" | "restored" | "recovery_required"; process_results: { identity: FocusProcessIdentity; outcome: string }[]; recovery_required: boolean; telemetry: { game_foreground: boolean; protection_triggered: boolean; total_cpu_basis_points: number; game_hot_thread_basis_points: number; competitor_count: number; max_competitor_basis_points: number } | null; adaptive_decision: { contention: string; action: string; priority_target_count: number; background_cpu_set_ids: number[]; game_cpu_selection: CpuSelection } | null }
export interface FocusExclusionReport { executable: string; excluded: boolean; pinned_executables: string[] }
export interface QosNormalizationReport { status: "no_change" | "applied"; prior: { execution_speed_throttled: boolean }; applied: { execution_speed_throttled: boolean }; restore_pending: boolean }
export interface PendingUpdate { version: string; notes: string | null; published_at: string | null }
export interface UpdateDownloadProgress { downloaded: string; total: string | null }
export interface CleanupSelection { wuwa: boolean; nvidia: boolean }
export interface CleanupPreview { token: string; selection: CleanupSelection; roots: { kind: string; path: string; files: number; bytes: number; skipped_entries: number }[]; warnings: string[] }
export interface CleanupReceipt { completed_at_unix: number; roots: { kind: string; outcome: string; deleted_files: number; deleted_bytes: number; skipped_entries: number; locked_entries: number; denied_entries: number; changed_entries: number; failed_entries: number }[]; receipt_persisted: boolean; stop_reason: string | null }
export interface CustomIniEntry { section: string; key: string; value: string; provenance: "custom"; runtime_verified: false }
export interface CustomProfile { schema_version: number; id: string; name: string; revision: number; patch: { schema_version: number; managed_ini: ManagedChange[]; custom_ini_entries: CustomIniEntry[]; process: { cpu_selection: CpuSelection; priority: PriorityClass } } }
export interface ProfileImportCandidate { token: string; preview: { display_name: string; patch: CustomProfile["patch"]; warnings: ("device_specific_cpu_reset" | "elevated_priority")[]; source_app_version: string; exported_at: string } }
export interface BackupSummary { id: string; created_at: string; sha256: string; reason: "first_original" | "preset" | "raw_editor" | "restore" | "manual"; pinned: boolean; integrity: string }
export type ExternalLinkKind = "source_code" | "releases" | "report_issue" | "kuro_games_official";
export interface RuntimeEventHandlers { onSupervisor?: (event: unknown) => void; onSupervisorError?: (error: { code: string }) => void; onFocusReport?: (report: FocusLifecycleReport) => void; onUpdateAvailable?: (update: { version: string }) => void }

export interface Commands {
  readonly preview: boolean;
  getAppSnapshot(): Promise<AppSnapshot>;
  previewIni(request: IniPreviewRequest): Promise<IniPreview>;
  previewIniImport(): Promise<IniPreview | null>;
  applyIni(token: string, confirmed: boolean): Promise<{ applied_sha256: string; backup_id: string }>;
  previewRestoreBackup(backupId: string): Promise<IniPreview>;
  restoreBackup(token: string, confirmed: boolean): Promise<{ applied_sha256: string; restored_from_id: string }>;
  saveProfile(profile: CustomProfile): Promise<CustomProfile>;
  discoverGame(): Promise<GameCandidate[]>;
  discoverGameManual(): Promise<GameCandidate | null>;
  selectGame(candidateToken: string, confirmed: boolean): Promise<void>;
  launchGame(): Promise<void>;
  getCpuTopology(): Promise<CpuTopology>;
  applyProcessSettings(request: ProcessSettingsRequest): Promise<ApplyReport>;
  previewFocusMode(): Promise<FocusPreviewEnvelope>;
  activateFocusMode(request: { token: string; selected: FocusProcessIdentity[]; select_all_eligible: boolean; select_all_confirmed: boolean }): Promise<FocusLifecycleReport>;
  deactivateFocusMode(): Promise<FocusLifecycleReport>;
  setFocusExclusion(token: string, candidate: FocusProcessIdentity, excluded: boolean): Promise<FocusExclusionReport>;
  normalizeGameQos(confirmed: boolean): Promise<QosNormalizationReport>;
  previewCacheCleanup(selection: CleanupSelection): Promise<CleanupPreview>;
  runCacheCleanup(token: string, confirmed: boolean): Promise<CleanupReceipt>;
  listProfiles(): Promise<CustomProfile[]>;
  exportProfile(id: string): Promise<boolean>;
  importProfile(): Promise<ProfileImportCandidate | null>;
  saveImportedProfile(token: string, id: string, name: string): Promise<CustomProfile>;
  listBackups(): Promise<BackupSummary[]>;
  pinBackup(backupId: string, pinned: boolean): Promise<void>;
  getPendingUpdate(): Promise<PendingUpdate | null>;
  installUpdate(confirmed: boolean, onProgress?: (progress: UpdateDownloadProgress) => void): Promise<void>;
  openExternalLink(kind: ExternalLinkKind): Promise<void>;
  subscribe(handlers: RuntimeEventHandlers): Promise<() => void>;
}

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export class CommandFailure extends Error {
  constructor(readonly code: string) {
    super(code);
    this.name = "CommandFailure";
  }
}

/** Normalizes Tauri rejections without trusting arbitrary backend text. */
export function normalizeCommandError(error: unknown): CommandFailure {
  if (typeof error === "object" && error !== null && "code" in error && typeof (error as { code?: unknown }).code === "string") return new CommandFailure((error as { code: string }).code);
  if (typeof error === "string" && /^[a-z0-9_]+$/.test(error)) return new CommandFailure(error);
  return new CommandFailure("unexpected_error");
}

export function commandErrorMessage(error: unknown, language: "en" | "ko" = "en"): string {
  const code = normalizeCommandError(error).code;
  const category = code.includes("stale") || code.includes("unknown") || code.includes("expired") ? "stale" : code.includes("external") || code.includes("changed") ? "conflict" : code.includes("running") || code.includes("active") ? "running" : code.includes("denied") || code.includes("access") ? "denied" : code.includes("invalid") || code.includes("validation") ? "validation" : "unexpected";
  const messages = {
    en: { stale: "The preview expired. Create a new preview.", conflict: "The source changed outside the app. Refresh and preview again.", running: "This operation is unavailable while the game is running.", denied: "Windows denied access. Review permissions and try again.", validation: "The selected values did not pass validation.", unexpected: "The operation failed. Try again or report the error code." },
    ko: { stale: "미리보기가 만료되었습니다. 새 미리보기를 만드세요.", conflict: "앱 외부에서 원본이 변경되었습니다. 새로고침 후 다시 미리보기 하세요.", running: "게임 실행 중에는 이 작업을 사용할 수 없습니다.", denied: "Windows가 접근을 거부했습니다. 권한을 확인하세요.", validation: "선택한 값이 검증을 통과하지 못했습니다.", unexpected: "작업에 실패했습니다. 다시 시도하거나 오류 코드를 제보하세요." },
  } as const;
  return `${messages[language][category]} (${code})`;
}

/** The only module that knows Tauri command names and outer argument casing. */
export function createTauriCommands(rawInvoke: Invoke = tauriInvoke): Commands {
  const invoke: Invoke = async <T,>(command: string, args?: Record<string, unknown>) => {
    try { return await rawInvoke<T>(command, args); } catch (error) { throw normalizeCommandError(error); }
  };
  return {
    preview: false,
    getAppSnapshot: () => invoke("get_app_snapshot"),
    previewIni: (request) => invoke("preview_ini", { request }),
    previewIniImport: () => invoke("preview_ini_import"),
    applyIni: (token, confirmed) => invoke("apply_ini", { token, confirmed }),
    previewRestoreBackup: (backupId) => invoke("preview_restore_backup", { backupId }),
    restoreBackup: (token, confirmed) => invoke("restore_backup", { token, confirmed }),
    saveProfile: (profile) => invoke("save_profile", { profile }),
    discoverGame: () => invoke("discover_game"),
    discoverGameManual: () => invoke("discover_game_manual"),
    selectGame: (candidateToken, confirmed) => invoke("select_game", { candidateToken, confirmed }),
    launchGame: () => invoke("launch_game"),
    getCpuTopology: () => invoke("get_cpu_topology"),
    applyProcessSettings: (request) => invoke("apply_process_settings", { request }),
    previewFocusMode: () => invoke("preview_focus_mode"),
    activateFocusMode: (request) => invoke("activate_focus_mode", { request }),
    deactivateFocusMode: () => invoke("deactivate_focus_mode"),
    setFocusExclusion: (token, candidate, excluded) => invoke("set_focus_exclusion", { request: { token, candidate, excluded } }),
    normalizeGameQos: (confirmed) => invoke("normalize_game_qos", { request: { disable_execution_speed_throttling: true, confirmed } }),
    previewCacheCleanup: (selection) => invoke("preview_cache_cleanup", { selection }),
    runCacheCleanup: (token, confirmed) => invoke("run_cache_cleanup", { token, confirmed }),
    listProfiles: () => invoke("list_profiles"),
    exportProfile: (id) => invoke("export_profile", { id }),
    importProfile: () => invoke("import_profile"),
    saveImportedProfile: (token, id, name) => invoke("save_imported_profile", { token, id, name }),
    listBackups: () => invoke("list_backups"),
    pinBackup: (backupId, pinned) => invoke("pin_backup", { backupId, pinned }),
    getPendingUpdate: () => invoke("get_pending_update"),
    installUpdate: async (confirmed, onProgress) => {
      const stop = onProgress ? await listen<UpdateDownloadProgress>("update://progress", (event) => onProgress(event.payload)) : null;
      try { await invoke("install_update", { confirmed }); } finally { stop?.(); }
    },
    openExternalLink: (kind) => invoke("open_external_link", { kind }),
    subscribe: async (handlers) => {
      const unlisten: UnlistenFn[] = [];
      if (handlers.onSupervisor) unlisten.push(await listen("supervisor://status", (e) => handlers.onSupervisor?.(e.payload)));
      if (handlers.onSupervisorError) unlisten.push(await listen<{ code: string }>("supervisor://error", (e) => handlers.onSupervisorError?.(e.payload)));
      if (handlers.onFocusReport) unlisten.push(await listen<FocusLifecycleReport>("focus://report", (e) => handlers.onFocusReport?.(e.payload)));
      if (handlers.onUpdateAvailable) unlisten.push(await listen<{ version: string }>("update://available", (e) => handlers.onUpdateAvailable?.(e.payload)));
      return () => unlisten.forEach((stop) => stop());
    },
  };
}

const installation: GameInstallation = { channel: "kuro", requires_user_confirmation: false, game_root: "C:\\Program Files\\Wuthering Waves\\Wuthering Waves Game", executable: "C:\\Program Files\\Wuthering Waves\\Wuthering Waves Game\\Wuthering Waves.exe", engine_ini: "C:\\Program Files\\Wuthering Waves\\Wuthering Waves Game\\Client\\Saved\\Config\\WindowsNoEditor\\Engine.ini" };
const cpuSets: CpuSetInfo[] = Array.from({ length: 8 }, (_, index) => ({ id: index, group: 0, logical_processor_index: index, core_index: Math.floor(index / 2), last_level_cache_index: 0, numa_node_index: 0, efficiency_class: index >= 4 ? 8 : 0, parked: false, allocated: false, allocated_to_target: false, realtime: false }));
const baseProfile: CustomProfile = { schema_version: 1, id: "balanced-custom", name: "Balanced Custom", revision: 1, patch: { schema_version: 1, managed_ini: [], custom_ini_entries: [], process: { cpu_selection: { mode: "all" }, priority: "normal" } } };

/** A deterministic adapter for browser previews and interaction tests. */
export function createFakeCommands(overrides: Partial<Commands> = {}): Commands {
  const makePreview = (text = "[SystemSettings]\nr.Tonemapper.Sharpen=1\n"): IniPreview => ({ token: "ini-preview-token", before_bytes: 36, after_bytes: text.length, candidate_text: text, diff: [{ kind: "metadata", old_line: null, new_line: null, text: "@@ Engine.ini @@" }, { kind: "added", old_line: null, new_line: 2, text: text.split("\n").find((line) => line.includes("=")) ?? "r.Tonemapper.Sharpen=1" }], semantic_changes: [{ section: "SystemSettings", key: "r.Tonemapper.Sharpen", before: "0", after: "1" }], before_encoding: "utf-8", after_encoding: "utf-8", before_line_endings: "crlf", after_line_endings: "crlf", byte_only_change: false });
  const focusReport: FocusLifecycleReport = { epoch: 3, process: null, status: "armed", process_results: [], recovery_required: false, telemetry: null, adaptive_decision: null };
  const fake: Commands = {
    preview: true,
    getAppSnapshot: async () => ({ version: "1.0.0", installation, supervisor_state: "idle", active_process: null, focus_runtime_availability: "available", focus_pinned_executables: [] }),
    previewIni: async (request) => makePreview(request.kind === "paste" ? request.text : `[SystemSettings]\n${request.changes.filter((c) => c.value !== null).map((c) => `${c.key}=${c.value}`).join("\n")}`),
    previewIniImport: async () => makePreview("[SystemSettings]\nr.MotionBlurQuality=0\n"),
    applyIni: async () => ({ applied_sha256: "demo-sha", backup_id: "backup-2" }),
    previewRestoreBackup: async () => makePreview("[SystemSettings]\nr.Tonemapper.Sharpen=0\n"),
    restoreBackup: async () => ({ applied_sha256: "restored-sha", restored_from_id: "backup-1" }),
    saveProfile: async (profile) => ({ ...profile, revision: profile.revision + 1 }),
    discoverGame: async () => [{ token: "candidate-token", installation }],
    discoverGameManual: async () => ({ token: "candidate-token", installation }),
    selectGame: async () => undefined,
    launchGame: async () => undefined,
    getCpuTopology: async () => ({ cpu_sets: cpuSets, groups: [{ group: 0, active_mask: "255" }] }),
    applyProcessSettings: async (request) => ({ status: "success", cpu: { requested: request.cpu_selection, applied: request.cpu_selection, error_code: null }, priority: { requested: request.priority, applied: request.priority, error_code: null } }),
    previewFocusMode: async () => ({ token: "focus-preview-uuid", thresholds: { sample_interval_ms: 1000, aggregate_contention_basis_points: 8000, release_basis_points: 6500, game_hot_thread_basis_points: 9000, competitor_basis_points: 500, sustained_samples: 3, release_samples: 3 }, runtime_availability: "available", candidates: [{ identity: { pid: 2248, creation_time_100ns: "42", canonical_image: "C:\\Apps\\Indexer.exe" }, display_name: "Indexer", current_priority: "normal", status: "eligible" }, { identity: { pid: 3020, creation_time_100ns: "43", canonical_image: "C:\\Apps\\Discord.exe" }, display_name: "Discord", current_priority: "normal", status: "communication" }, { identity: { pid: 3100, creation_time_100ns: "44", canonical_image: "C:\\Apps\\obs64.exe" }, display_name: "OBS Studio", current_priority: "normal", status: "recording" }, { identity: { pid: 3200, creation_time_100ns: "45", canonical_image: "C:\\Apps\\Streamlabs.exe" }, display_name: "Streamlabs", current_priority: "normal", status: "streaming" }, { identity: { pid: 3300, creation_time_100ns: "46", canonical_image: "C:\\Apps\\XSplit.exe" }, display_name: "XSplit", current_priority: "normal", status: "capture_overlay" }] }),
    activateFocusMode: async () => focusReport,
    deactivateFocusMode: async () => ({ ...focusReport, status: "restored" }),
    setFocusExclusion: async (_token, candidate, excluded) => ({ executable: candidate.canonical_image, excluded, pinned_executables: excluded ? [candidate.canonical_image] : [] }),
    normalizeGameQos: async () => ({ status: "applied", prior: { execution_speed_throttled: true }, applied: { execution_speed_throttled: false }, restore_pending: true }),
    previewCacheCleanup: async (selection) => ({ token: "cache-preview-token", selection, roots: [{ kind: "wuwa_pso", path: "…\\Saved\\PSO", files: 14, bytes: 8300000, skipped_entries: 0 }], warnings: ["troubleshooting_only", "shader_rebuild_may_stutter", ...(selection.nvidia ? ["nvidia_cache_is_driver_wide"] : [])] }),
    runCacheCleanup: async () => ({ completed_at_unix: 1784052000, roots: [{ kind: "wuwa_pso", outcome: "complete", deleted_files: 14, deleted_bytes: 8300000, skipped_entries: 0, locked_entries: 0, denied_entries: 0, changed_entries: 0, failed_entries: 0 }], receipt_persisted: true, stop_reason: null }),
    listProfiles: async () => [baseProfile],
    exportProfile: async () => true,
    importProfile: async () => ({ token: "profile-import-token", preview: { display_name: "Shared Performance", patch: baseProfile.patch, warnings: ["device_specific_cpu_reset"], source_app_version: "1.0.0", exported_at: "2026-07-14T10:00:00Z" } }),
    saveImportedProfile: async (_token, id, name) => ({ ...baseProfile, id, name }),
    listBackups: async () => [{ id: "backup-1", created_at: "2026-07-14T10:30:00Z", sha256: "932ca…f12", reason: "raw_editor", pinned: true, integrity: "verified" }],
    pinBackup: async () => undefined,
    getPendingUpdate: async () => null,
    installUpdate: async () => undefined,
    openExternalLink: async () => undefined,
    subscribe: async () => () => undefined,
  };
  return { ...fake, ...overrides };
}

export const commands = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window ? createTauriCommands() : createFakeCommands();
