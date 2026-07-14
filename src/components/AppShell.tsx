import { CircleGauge, Cpu, FileCode2, FolderClock, Info, Settings, SlidersHorizontal, UserRoundCog } from "lucide-react";
import { useMemo, type ComponentType, type ReactNode } from "react";
import type { LucideProps } from "lucide-react";
import { UpdatePrompt } from "../features/update/UpdatePrompt";
import { createUpdateService } from "../features/update/update-service";
import { AppIcon } from "./AppIcon";
import { useAppState, type PageId } from "../state/AppState";

const navigation: { id: PageId; label: string; ko: string; icon: ComponentType<LucideProps> }[] = [
  { id: "home", label: "Dashboard", ko: "홈", icon: CircleGauge },
  { id: "engine", label: "Engine.ini", ko: "Engine.ini", icon: FileCode2 },
  { id: "cpu", label: "CPU & Priority", ko: "CPU 및 우선도", icon: Cpu },
  { id: "profiles", label: "Profiles", ko: "프로필", icon: UserRoundCog },
  { id: "backups", label: "Backups", ko: "백업", icon: FolderClock },
  { id: "settings", label: "Settings", ko: "설정", icon: Settings },
  { id: "about", label: "About", ko: "정보", icon: Info },
];

export function AppShell({ children }: { children: ReactNode }) {
  const { page, setPage, snapshot, commands, text, language, updateVersion, clearUpdate, writeInProgress, maintenanceInProgress } = useAppState();
  const updater = useMemo(() => createUpdateService({ getPendingUpdate: commands.getPendingUpdate, installUpdate: commands.installUpdate }), [commands]);
  return <div className="app-shell">
    <aside className="side-rail"><div className="brand"><AppIcon size={30} /><div><strong>Wuwa ini Tool</strong><span>v{snapshot?.version ?? "1.0.0"}</span></div>{commands.preview ? <span className="preview-badge">Preview</span> : null}</div>
      <nav aria-label={text("Primary navigation", "주요 탐색")}>{navigation.map(({ id, label, ko, icon: Icon }) => <button aria-current={page === id ? "page" : undefined} key={id} onClick={() => setPage(id)}><Icon aria-hidden="true" size={19} /><span>{text(label, ko)}</span></button>)}</nav>
      <div className="rail-footer"><SlidersHorizontal aria-hidden="true" size={18} /><span>{snapshot?.installation ? text("Game configured", "게임 설정됨") : text("Setup required", "설정 필요")}</span></div>
    </aside>
    <main className="app-main" data-page={page}>
      <UpdatePrompt announcedVersion={updateVersion} gameRunning={snapshot?.active_process != null} language={language} maintenanceInProgress={maintenanceInProgress} onDefer={clearUpdate} updater={updater} writeInProgress={writeInProgress} />
      {children}
    </main>
  </div>;
}
