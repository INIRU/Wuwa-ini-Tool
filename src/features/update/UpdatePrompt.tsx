import { Download, LoaderCircle, Timer } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "../../components/Button";
import { Notice } from "../../components/shared";
import type { UpdateInfo, UpdateProgress, UpdateService } from "./update-service";

export interface UpdatePromptProps {
  updater: UpdateService;
  announcedVersion?: string | null;
  writeInProgress: boolean;
  maintenanceInProgress: boolean;
  gameRunning: boolean;
  language?: "en" | "ko";
  onDefer?: () => void;
}

type PromptPhase = "checking" | "ready" | UpdateProgress["phase"] | "failed" | "deferred";

const messages = {
  en: {
    title: "Update available",
    version: "Version",
    notes: "Release notes",
    notesUnavailable: "Release notes are unavailable.",
    updateNow: "Update now",
    later: "Later",
    checking: "Checking for updates…",
    installing: "Installing update…",
    downloading: "Downloading update…",
    restarting: "Restarting the app…",
    checkFailed: "Update check unavailable",
    checkFailedDetail: "You can keep using the app and try again later.",
    installFailed: "Update could not be installed",
    installFailedDetail: "Nothing was changed. Close the game and try again.",
    restartNote: "The signed update is installed only after approval. The app restarts when installation finishes.",
    writeBlocked: "Finish the current configuration write before updating.",
    maintenanceBlocked: "Finish cache or maintenance work before updating.",
    gameBlocked: "Close Wuthering Waves before updating.",
  },
  ko: {
    title: "업데이트 사용 가능",
    version: "버전",
    notes: "릴리스 노트",
    notesUnavailable: "릴리스 노트를 불러올 수 없습니다.",
    updateNow: "지금 업데이트",
    later: "나중에",
    checking: "업데이트 확인 중…",
    installing: "업데이트 설치 중…",
    downloading: "업데이트 다운로드 중…",
    restarting: "앱을 다시 시작하는 중…",
    checkFailed: "업데이트를 확인할 수 없음",
    checkFailedDetail: "앱은 계속 사용할 수 있으며 나중에 다시 시도할 수 있습니다.",
    installFailed: "업데이트를 설치하지 못함",
    installFailedDetail: "변경된 내용은 없습니다. 게임을 종료하고 다시 시도하세요.",
    restartNote: "서명된 업데이트는 승인한 뒤에만 설치되며, 설치가 끝나면 앱이 다시 시작됩니다.",
    writeBlocked: "현재 설정 쓰기가 끝난 뒤 업데이트하세요.",
    maintenanceBlocked: "캐시 정리 또는 유지보수 작업이 끝난 뒤 업데이트하세요.",
    gameBlocked: "명조를 종료한 뒤 업데이트하세요.",
  },
} as const;

export function UpdatePrompt({
  updater,
  announcedVersion = null,
  writeInProgress,
  maintenanceInProgress,
  gameRunning,
  language = "en",
  onDefer,
}: UpdatePromptProps) {
  const copy = messages[language];
  const [phase, setPhase] = useState<PromptPhase>("checking");
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [installFailed, setInstallFailed] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState<Extract<UpdateProgress, { phase: "downloading" }> | null>(null);

  useEffect(() => {
    let active = true;
    setPhase("checking");
    void updater.check().then((discovered) => {
      if (!active) return;
      if (!discovered || updater.isDeferred(discovered.version)) {
        setUpdate(null);
        setPhase("deferred");
        return;
      }
      setUpdate(discovered);
      setPhase("ready");
    }).catch(() => {
      if (active) setPhase("failed");
    });
    return () => { active = false; };
  }, [announcedVersion, updater]);

  if (phase === "checking") {
    return announcedVersion ? <div className="notice" role="status"><LoaderCircle aria-hidden="true" className="spin" size={18} />{copy.checking}</div> : null;
  }
  if (phase === "failed") {
    return <Notice><strong>{copy.checkFailed}</strong><span>{copy.checkFailedDetail}</span></Notice>;
  }
  if (!update || phase === "deferred") return null;

  const blockedReason = writeInProgress
    ? copy.writeBlocked
    : maintenanceInProgress
      ? copy.maintenanceBlocked
      : gameRunning
        ? copy.gameBlocked
        : null;
  const busy = phase === "installing" || phase === "downloading" || phase === "restarting";
  const downloadPercent = downloadProgress?.total && BigInt(downloadProgress.total) > 0n
    ? Number((BigInt(downloadProgress.downloaded) * 100n) / BigInt(downloadProgress.total))
    : null;

  const install = async () => {
    try {
      setInstallFailed(false);
      setDownloadProgress(null);
      setPhase("installing");
      await updater.downloadAndInstall((progress) => {
        if (progress.phase === "downloading") setDownloadProgress(progress);
        setPhase(progress.phase);
      });
    } catch {
      setPhase("ready");
      setInstallFailed(true);
    }
  };

  const defer = () => {
    updater.defer(update.version);
    setPhase("deferred");
    onDefer?.();
  };

  return (
    <aside aria-labelledby="update-prompt-title" className="section">
      <div className="section-heading">
        <div>
          <h2 id="update-prompt-title">{copy.title}</h2>
          <p><strong>{copy.version}</strong> <code>{update.version}</code></p>
        </div>
        <span className={busy ? "status status--active" : "status"} role="status">
          {phase === "installing" ? copy.installing : phase === "downloading" ? `${copy.downloading}${downloadPercent === null ? "" : ` ${downloadPercent}%`}` : phase === "restarting" ? copy.restarting : copy.title}
        </span>
      </div>
      {phase === "downloading" ? <progress aria-label={copy.downloading} max={100} value={downloadPercent ?? undefined} /> : null}
      <h3>{copy.notes}</h3>
      <p className="muted">{update.notes?.trim() || copy.notesUnavailable}</p>
      <Notice>{copy.restartNote}</Notice>
      {blockedReason ? <Notice kind="warning">{blockedReason}</Notice> : null}
      {installFailed ? <Notice kind="warning"><strong>{copy.installFailed}</strong><span>{copy.installFailedDetail}</span></Notice> : null}
      <div className="action-row">
        <Button disabled={busy || blockedReason !== null} onClick={() => void install()} variant="primary">
          {busy ? <LoaderCircle aria-hidden="true" className="spin" size={17} /> : <Download aria-hidden="true" size={17} />}
          {copy.updateNow}
        </Button>
        <Button disabled={busy} onClick={defer}>
          <Timer aria-hidden="true" size={17} />{copy.later}
        </Button>
      </div>
    </aside>
  );
}
