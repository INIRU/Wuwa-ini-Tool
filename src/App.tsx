import { useState } from "react";
import type { Commands } from "./api/commands";
import { commands as defaultCommands } from "./api/commands";
import { AppShell } from "./components/AppShell";
import { Button } from "./components/Button";
import { Notice } from "./components/shared";
import { AboutPage } from "./pages/AboutPage";
import { BackupsPage } from "./pages/BackupsPage";
import { CpuPage } from "./pages/CpuPage";
import { EnginePage } from "./pages/EnginePage";
import { HomePage } from "./pages/HomePage";
import { ProfilesPage } from "./pages/ProfilesPage";
import { SettingsPage } from "./pages/SettingsPage";
import { AppStateProvider, useAppState } from "./state/AppState";

function CurrentPage() {
  const { page } = useAppState();
  if (page === "engine") return <EnginePage />;
  if (page === "cpu") return <CpuPage />;
  if (page === "profiles") return <ProfilesPage />;
  if (page === "backups") return <BackupsPage />;
  if (page === "settings") return <SettingsPage />;
  if (page === "about") return <AboutPage />;
  return <HomePage />;
}

function Disclaimer({ onAccept }: { onAccept: () => void }) {
  const [checked, setChecked] = useState(false);
  return <div className="modal-layer"><section aria-labelledby="notice-title" aria-modal="true" className="disclaimer" role="dialog"><span className="eyebrow">FIRST RUN / 최초 실행</span><h1 id="notice-title">Important notice · 중요 안내</h1><p>Wuwa ini Tool is an independent open-source utility and is not affiliated with Kuro Games.</p><p>Wuwa ini Tool은 Kuro Games와 관련 없는 독립 오픈 소스 도구입니다.</p><Notice kind="warning"><b>Use at your own risk / 사용자 판단 필요</b><span>Engine.ini and process changes may cause game crashes, lost settings, instability, or account restrictions. No setting is guaranteed safe or effective. This notice does not automatically eliminate legal duties or liability that cannot lawfully be excluded.</span><span>Engine.ini 및 프로세스 변경은 게임 충돌, 설정 손실, 불안정, 게임 이용 제재를 일으킬 수 있으며 안전성이나 효과를 보장하지 않습니다. 이 고지만으로 법률상 배제할 수 없는 책임이 자동 소멸하지 않습니다.</span></Notice><label className="check-row"><input checked={checked} onChange={(event) => setChecked(event.target.checked)} type="checkbox" />I understand and accept this notice / 위 내용을 이해하고 동의합니다</label><Button disabled={!checked} variant="primary" onClick={() => { localStorage.setItem("wuwa-disclaimer-v1", "accepted"); onAccept(); }}>Continue / 계속</Button></section></div>;
}

export function App({ commands = defaultCommands }: { commands?: Commands }) {
  const [accepted, setAccepted] = useState(() => localStorage.getItem("wuwa-disclaimer-v1") === "accepted");
  if (!accepted) return <Disclaimer onAccept={() => setAccepted(true)} />;
  return <AppStateProvider commands={commands}><AppShell><CurrentPage /></AppShell></AppStateProvider>;
}
