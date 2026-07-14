import { Pin, PinOff, RotateCcw } from "lucide-react";
import { useState } from "react";
import { commandErrorMessage, type IniPreview } from "../api/commands";
import { Button } from "../components/Button";
import { DiffView } from "../components/DiffView";
import { EmptyState, Notice, PageHeader } from "../components/shared";
import { useAppState } from "../state/AppState";

export function BackupsPage() {
  const { commands, backups, refresh, text, language, setWriteInProgress } = useAppState();
  const [preview, setPreview] = useState<IniPreview | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const reasonLabel = (reason: (typeof backups)[number]["reason"]) => text(reason.replace(/_/g, " "), ({ first_original: "최초 원본", preset: "프리셋", raw_editor: "원본 편집", restore: "복원", manual: "수동" } as const)[reason]);
  const integrityLabel = (integrity: string) => text(integrity, ({ verified: "검증됨", corrupt: "손상됨", missing: "누락됨" } as Record<string, string>)[integrity] ?? integrity);
  const restore = async () => { if (!preview) return; setWriteInProgress(true); try { const result = await commands.restoreBackup(preview.token, confirmed); setMessage(text(`Restored from ${result.restored_from_id}. A safety backup was created first.`, `${result.restored_from_id}에서 복원했으며 먼저 안전 백업을 생성했습니다.`)); setPreview(null); setConfirmed(false); await refresh(); } catch (error) { setMessage(commandErrorMessage(error, language)); setPreview(null); } finally { setWriteInProgress(false); } };
  const togglePin = async (backupId: string, pinned: boolean) => { try { await commands.pinBackup(backupId, pinned); await refresh(); } catch (error) { setMessage(commandErrorMessage(error, language)); } };
  const previewRestore = async (backupId: string) => { setPreview(null); setConfirmed(false); try { setPreview(await commands.previewRestoreBackup(backupId)); } catch (error) { setMessage(commandErrorMessage(error, language)); } };
  return <div className="page"><PageHeader title={text("Backups", "백업")} description={text("Integrity-checked Engine.ini history. Pin important records and preview every restore before writing.", "무결성을 확인한 Engine.ini 기록입니다. 중요한 항목을 고정하고 복원 전 diff를 확인하세요.")} />
    {message ? <Notice kind={message.startsWith("Restored") ? "success" : "warning"}>{message}</Notice> : null}
    <section className="section"><h2>{text("Backup history", "백업 기록")}</h2>{backups.length ? <div className="list">{backups.map((backup) => <article className="list-row" key={backup.id}><div><b>{new Date(backup.created_at).toLocaleString()}</b><span>{reasonLabel(backup.reason)} · {integrityLabel(backup.integrity)}</span></div><div className="list-row__meta"><code>{backup.sha256}</code>{backup.pinned ? <span>{text("Pinned", "고정됨")}</span> : null}</div><div className="action-row"><Button onClick={() => void togglePin(backup.id, !backup.pinned)}>{backup.pinned ? <PinOff aria-hidden="true" size={17} /> : <Pin aria-hidden="true" size={17} />}{backup.pinned ? text("Unpin", "고정 해제") : text("Pin", "고정")}</Button><Button disabled={backup.integrity !== "verified"} onClick={() => void previewRestore(backup.id)}><RotateCcw aria-hidden="true" size={17} />{text("Preview restore", "복원 미리보기")}</Button></div></article>)}</div> : <EmptyState title={text("No backups", "백업 없음")} description={text("The first Engine.ini apply creates an original backup automatically.", "Engine.ini를 처음 적용할 때 원본 백업을 자동 생성합니다.")} />}</section>
    {preview ? <section className="apply-panel"><h2>{text("Restore preview", "복원 미리보기")}</h2><DiffView preview={preview} /><label className="check-row"><input checked={confirmed} onChange={(e) => setConfirmed(e.target.checked)} type="checkbox" />{text("Confirm safety backup and restore this exact preview", "안전 백업 후 이 미리보기 복원을 확인합니다")}</label><Button disabled={!confirmed} variant="primary" onClick={() => void restore()}>{text("Create backup and restore", "백업 생성 후 복원")}</Button></section> : null}
  </div>;
}
