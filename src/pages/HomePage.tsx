import { Play, RefreshCw, Search } from "lucide-react";
import { useState } from "react";
import { commandErrorMessage, type GameCandidate } from "../api/commands";
import { Button } from "../components/Button";
import { LoadingState, Notice, PageHeader } from "../components/shared";
import { useAppState } from "../state/AppState";

export function HomePage() {
  const { commands, snapshot, loading, partialErrors, refresh, setPage, text, language } = useAppState();
  const [message, setMessage] = useState<string | null>(null);
  const [candidates, setCandidates] = useState<GameCandidate[]>([]);
  const [candidateConfirmed, setCandidateConfirmed] = useState<string | null>(null);
  const partialErrorLabel = (code: string) => text(code.replace(/_/g, " "), ({ app_snapshot_unavailable: "앱 상태 사용 불가", profile_list_unavailable: "프로필 목록 사용 불가", backup_list_unavailable: "백업 목록 사용 불가", cpu_topology_unavailable: "CPU 토폴로지 사용 불가", runtime_events_unavailable: "실시간 상태 이벤트 사용 불가" } as Record<string, string>)[code] ?? code.replace(/_/g, " "));
  const discover = async () => {
    try { const candidates = await commands.discoverGame(); setCandidates(candidates); setMessage(candidates.length ? text(`Found ${candidates.length} validated installation.`, `검증된 설치 경로 ${candidates.length}개를 찾았습니다.`) : text("No installation was found. Use manual selection from Settings.", "설치 경로를 찾지 못했습니다. 설정에서 직접 선택하세요.")); } catch (error) { setMessage(commandErrorMessage(error, language)); }
  };
  const selectCandidate = async (candidate: GameCandidate) => { try { await commands.selectGame(candidate.token, true); setCandidates([]); setCandidateConfirmed(null); await refresh(); } catch (error) { setMessage(commandErrorMessage(error, language)); } };
  const launch = async () => { try { await commands.launchGame(); setMessage(text("Launch requested.", "게임 실행을 요청했습니다.")); } catch (error) { setMessage(commandErrorMessage(error, language)); } };
  if (loading && !snapshot) return <LoadingState label={text("Loading application state…", "앱 상태를 불러오는 중…")} />;
  return <div className="page"><PageHeader title={text("Dashboard", "대시보드")} description={text("A clear view of the selected installation, game process, and configuration safety.", "선택한 설치 경로, 게임 프로세스, 설정 안전 상태를 한눈에 확인합니다.")} actions={<Button onClick={() => void refresh()}><RefreshCw aria-hidden="true" size={17} />{text("Refresh", "새로고침")}</Button>} />
    {partialErrors.length ? <Notice kind="warning">{text("Some data could not be loaded", "일부 데이터를 불러오지 못했습니다")}: {partialErrors.map(partialErrorLabel).join(", ")}</Notice> : null}
    {message ? <Notice>{message}</Notice> : null}
    {candidates.length ? <section className="section"><h2>{text("Validated installations", "검증된 설치 경로")}</h2>{candidates.map((candidate) => <div className="confirm-panel" key={candidate.token}><b>{candidate.installation.channel === "steam" ? "Steam" : candidate.installation.channel === "kuro" ? "Kuro Games launcher" : text("Manual selection", "직접 선택")}</b><code>{candidate.installation.executable}</code><code>{candidate.installation.engine_ini}</code><label className="check-row"><input checked={candidateConfirmed === candidate.token} onChange={(event) => setCandidateConfirmed(event.target.checked ? candidate.token : null)} type="checkbox" />{text("Confirm this executable and Engine.ini path", "이 실행 파일과 Engine.ini 경로를 확인합니다")}</label><Button disabled={candidateConfirmed !== candidate.token} onClick={() => void selectCandidate(candidate)}>{text("Use this installation", "이 설치 경로 사용")}</Button></div>)}</section> : null}
    <section className="summary-panel"><div className="summary-panel__title"><div><span className="eyebrow">{text("INSTALLATION", "설치")}</span><h2>{snapshot?.installation ? "Wuthering Waves" : text("Game not configured", "게임이 설정되지 않음")}</h2></div><span className={`status status--${snapshot?.supervisor_state ?? "idle"}`}>{snapshot ? text(snapshot.supervisor_state.replace(/_/g, " "), ({ idle: "대기", launching: "실행 중", waiting_for_game: "게임 대기", applying: "적용 중", active: "활성", partial: "부분 적용", denied: "거부됨", exited: "종료됨" } as const)[snapshot.supervisor_state]) : text("unavailable", "사용 불가")}</span></div>
      <dl className="details"><div><dt>Engine.ini</dt><dd>{snapshot?.installation?.engine_ini ?? text("Select a validated game executable first", "먼저 검증된 게임 실행 파일을 선택하세요")}</dd></div><div><dt>{text("Executable", "실행 파일")}</dt><dd>{snapshot?.installation?.executable ?? text("Not selected", "선택되지 않음")}</dd></div><div><dt>{text("Process", "프로세스")}</dt><dd>{snapshot?.active_process ? `PID ${snapshot.active_process.pid}` : text("Not running", "실행 중이 아님")}</dd></div></dl>
      <div className="action-row">{snapshot?.installation ? <><Button variant="primary" onClick={() => void launch()}><Play aria-hidden="true" size={17} />{text("Launch game", "게임 실행")}</Button><Button onClick={() => setPage("engine")}>{text("Open Engine.ini", "Engine.ini 열기")}</Button></> : <Button variant="primary" onClick={() => void discover()}><Search aria-hidden="true" size={17} />{text("Discover game", "게임 찾기")}</Button>}</div>
    </section>
    <section className="section"><h2>{text("Safe workflow", "안전한 작업 흐름")}</h2><ol className="workflow"><li><b>{text("Preview", "미리보기")}</b><span>{text("Every file or process change starts with a read-only preview.", "모든 파일 및 프로세스 변경은 읽기 전용 미리보기로 시작합니다.")}</span></li><li><b>{text("Back up", "백업")}</b><span>{text("Engine.ini is backed up before an atomic write.", "원자적 쓰기 전에 Engine.ini를 백업합니다.")}</span></li><li><b>{text("Verify", "검증")}</b><span>{text("Readback and receipts show complete, partial, or denied results.", "읽기 결과와 영수증으로 완료·부분·거부 상태를 표시합니다.")}</span></li></ol></section>
  </div>;
}
