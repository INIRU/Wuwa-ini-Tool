import { useState } from "react";
import type { IniPreview } from "../api/commands";
import { useAppState } from "../state/AppState";

export function DiffView({ preview }: { preview: IniPreview }) {
  const [mode, setMode] = useState<"unified" | "split">("unified");
  const { text } = useAppState();
  const splitAvailable = preview.semantic_changes.length > 0;
  return <section className="diff" aria-label="Engine.ini diff" role="region">
    <header className="diff__header"><strong>Engine.ini diff</strong><div className="segmented" aria-label={text("Diff layout", "diff 배치")}><button aria-pressed={mode === "unified"} onClick={() => setMode("unified")}>{text("Unified", "통합")}</button><button aria-pressed={mode === "split"} disabled={!splitAvailable} title={splitAvailable ? undefined : text("Split view requires semantic changes", "분할 보기는 의미 변경 정보가 필요합니다")} onClick={() => setMode("split")}>{text("Split", "분할")}</button></div></header>
    <div className={`diff__body diff__body--${mode}`}>
      {mode === "unified" ? preview.diff.map((line, index) => <code className={`diff-line diff-line--${line.kind}`} key={`${index}-${line.text}`}><span>{line.old_line ?? ""}</span><span>{line.new_line ?? ""}</span><b>{line.kind === "added" ? "+" : line.kind === "removed" ? "−" : " "}</b>{line.text}</code>) : <><div><b>{text("Before", "변경 전")}</b>{preview.semantic_changes.map((change) => <code key={change.key}>{change.key}={change.before ?? "∅"}</code>)}</div><div><b>{text("After", "변경 후")}</b>{preview.semantic_changes.map((change) => <code key={change.key}>{change.key}={change.after ?? "∅"}</code>)}</div></>}
    </div>
    <footer className="diff__meta">{preview.before_bytes} → {preview.after_bytes} {text("bytes", "바이트")} · {preview.before_encoding} → {preview.after_encoding} · {preview.before_line_endings} → {preview.after_line_endings}{preview.byte_only_change ? text(" · byte-only change", " · 바이트만 변경됨") : ""}{!splitAvailable ? text(" · unified-only full-file diff", " · 통합 보기 전용 전체 파일 diff") : ""}</footer>
  </section>;
}
