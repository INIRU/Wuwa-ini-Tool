import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import type { AppLanguage, ThemeMode } from "../contracts";
import type { AppSnapshot, BackupSummary, Commands, CpuTopology, CustomProfile, FocusLifecycleReport } from "../api/commands";

export type PageId = "home" | "engine" | "cpu" | "profiles" | "backups" | "settings" | "about";

interface AppStateValue {
  commands: Commands;
  page: PageId;
  setPage: (page: PageId) => void;
  snapshot: AppSnapshot | null;
  profiles: CustomProfile[];
  backups: BackupSummary[];
  topology: CpuTopology | null;
  loading: boolean;
  partialErrors: string[];
  writeInProgress: boolean;
  setWriteInProgress: (active: boolean) => void;
  maintenanceInProgress: boolean;
  setMaintenanceInProgress: (active: boolean) => void;
  refresh: () => Promise<void>;
  language: AppLanguage;
  setLanguage: (language: AppLanguage) => void;
  text: (english: string, korean: string) => string;
  theme: ThemeMode;
  setTheme: (theme: ThemeMode) => void;
  updateVersion: string | null;
  clearUpdate: () => void;
  focusReport: FocusLifecycleReport | null;
  setFocusReport: (report: FocusLifecycleReport | null) => void;
  focusEnabled: boolean;
  setFocusEnabled: (enabled: boolean) => void;
  focusRestoring: boolean;
  setFocusRestoring: (restoring: boolean) => void;
  focusError: string | null;
  setFocusError: (code: string | null) => void;
  clearFocusError: () => void;
  profileDraft: CustomProfile["patch"];
  setEngineDraft: (managed: CustomProfile["patch"]["managed_ini"], custom: CustomProfile["patch"]["custom_ini_entries"]) => void;
  setProcessDraft: (process: CustomProfile["patch"]["process"]) => void;
}

const AppStateContext = createContext<AppStateValue | null>(null);

function readSetting<T extends string>(key: string, fallback: T): T {
  return (localStorage.getItem(key) as T | null) ?? fallback;
}

export function AppStateProvider({ children, commands }: { children: ReactNode; commands: Commands }) {
  const [page, setPage] = useState<PageId>("home");
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [profiles, setProfiles] = useState<CustomProfile[]>([]);
  const [backups, setBackups] = useState<BackupSummary[]>([]);
  const [topology, setTopology] = useState<CpuTopology | null>(null);
  const [loading, setLoading] = useState(true);
  const [partialErrors, setPartialErrors] = useState<string[]>([]);
  const [writeInProgress, setWriteInProgress] = useState(false);
  const [maintenanceInProgress, setMaintenanceInProgress] = useState(false);
  const [language, setLanguageState] = useState<AppLanguage>(() => readSetting("wuwa-language", navigator.language.toLowerCase().startsWith("ko") ? "ko" : "en"));
  const [theme, setThemeState] = useState<ThemeMode>(() => readSetting("wuwa-theme", "system"));
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [focusReport, setFocusReport] = useState<FocusLifecycleReport | null>(null);
  const [focusEnabled, setFocusEnabled] = useState(false);
  const [focusRestoring, setFocusRestoring] = useState(false);
  const [focusError, setFocusError] = useState<string | null>(null);
  const [profileDraft, setProfileDraft] = useState<CustomProfile["patch"]>({ schema_version: 1, managed_ini: [], custom_ini_entries: [], process: { cpu_selection: { mode: "all" }, priority: "normal" } });

  const refresh = async () => {
    setLoading(true);
    const results = await Promise.allSettled([
      commands.getAppSnapshot(),
      commands.listProfiles(),
      commands.listBackups(),
      commands.getCpuTopology(),
    ]);
    const errors: string[] = [];
    if (results[0].status === "fulfilled") setSnapshot(results[0].value); else errors.push("app_snapshot_unavailable");
    if (results[1].status === "fulfilled") setProfiles(results[1].value); else errors.push("profile_list_unavailable");
    if (results[2].status === "fulfilled") setBackups(results[2].value); else errors.push("backup_list_unavailable");
    if (results[3].status === "fulfilled") setTopology(results[3].value); else errors.push("cpu_topology_unavailable");
    setPartialErrors(errors);
    setLoading(false);
  };

  useEffect(() => { void refresh(); }, [commands]);
  useEffect(() => {
    let stop: (() => void) | undefined;
    void commands.subscribe({
      onSupervisor: () => void commands.getAppSnapshot().then(setSnapshot).catch(() => setPartialErrors((errors) => [...new Set([...errors, "app_snapshot_unavailable"])])),
      onSupervisorError: ({ code }) => { setFocusError(code); setFocusRestoring(false); },
      onFocusReport: (report) => {
        setFocusReport(report);
        setFocusEnabled(["activated", "armed", "applied", "recovery_required"].includes(report.status));
        setFocusRestoring(false);
        setFocusError(null);
      },
      onUpdateAvailable: ({ version }) => setUpdateVersion(version),
    }).then((unlisten) => { stop = unlisten; }).catch(() => setPartialErrors((errors) => [...new Set([...errors, "runtime_events_unavailable"])]));
    return () => stop?.();
  }, [commands]);
  useEffect(() => {
    if (theme === "system") document.documentElement.removeAttribute("data-theme");
    else document.documentElement.dataset.theme = theme;
  }, [theme]);

  const setLanguage = (value: AppLanguage) => { localStorage.setItem("wuwa-language", value); setLanguageState(value); };
  const setTheme = (value: ThemeMode) => { localStorage.setItem("wuwa-theme", value); setThemeState(value); };
  const setEngineDraft = (managed: CustomProfile["patch"]["managed_ini"], custom: CustomProfile["patch"]["custom_ini_entries"]) => setProfileDraft((draft) => ({ ...draft, managed_ini: managed, custom_ini_entries: custom }));
  const setProcessDraft = (process: CustomProfile["patch"]["process"]) => setProfileDraft((draft) => ({ ...draft, process }));
  const value = useMemo<AppStateValue>(() => ({ commands, page, setPage, snapshot, profiles, backups, topology, loading, partialErrors, writeInProgress, setWriteInProgress, maintenanceInProgress, setMaintenanceInProgress, refresh, language, setLanguage, text: (english, korean) => language === "ko" ? korean : english, theme, setTheme, updateVersion, clearUpdate: () => setUpdateVersion(null), focusReport, setFocusReport, focusEnabled, setFocusEnabled, focusRestoring, setFocusRestoring, focusError, setFocusError, clearFocusError: () => setFocusError(null), profileDraft, setEngineDraft, setProcessDraft }), [commands, page, snapshot, profiles, backups, topology, loading, partialErrors, writeInProgress, maintenanceInProgress, language, theme, updateVersion, focusReport, focusEnabled, focusRestoring, focusError, profileDraft]);
  return <AppStateContext.Provider value={value}>{children}</AppStateContext.Provider>;
}

export function useAppState() {
  const value = useContext(AppStateContext);
  if (!value) throw new Error("useAppState must be used inside AppStateProvider");
  return value;
}
