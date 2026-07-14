import { Code2, ExternalLink, Scale } from "lucide-react";
import { Button } from "../components/Button";
import { Notice, PageHeader } from "../components/shared";
import { useAppState } from "../state/AppState";

export function AboutPage() {
  const { commands, snapshot, text } = useAppState();
  return <div className="page"><PageHeader title={text("About", "정보")} description={`Wuwa ini Tool ${snapshot?.version ?? "1.0.0"} · ${text("open-source Windows utility", "오픈 소스 Windows 도구")}`} />
    <section className="section about"><Scale aria-hidden="true" size={32} /><h2>{text("Independent community tool", "독립 커뮤니티 도구")}</h2><p>{text("This application is not affiliated with, endorsed by, or supported by Kuro Games. Engine.ini behavior can change between game versions.", "이 앱은 Kuro Games와 관련되거나 보증·지원받지 않습니다. Engine.ini 동작은 게임 버전에 따라 바뀔 수 있습니다.")}</p><Notice kind="warning">{text("There is no guarantee that a setting is safe or effective. Crashes, lost settings, instability, and account restrictions are possible. This notice does not automatically eliminate liability that cannot legally be excluded.", "설정의 안전성이나 효과를 보장하지 않습니다. 충돌, 설정 손실, 불안정, 계정 제재가 발생할 수 있습니다. 이 고지만으로 법률상 배제할 수 없는 책임이 자동 소멸하지 않습니다.")}</Notice><p>{text("Source code and English comments are public so users can inspect behavior, report issues, and submit pull requests.", "동작 검토, 이슈 제보, Pull Request를 위해 소스 코드와 영어 주석을 공개합니다.")}</p><div className="action-row"><Button onClick={() => void commands.openExternalLink("source_code").catch(() => undefined)}><Code2 aria-hidden="true" size={17} />{text("Source code", "소스 코드")}</Button><Button onClick={() => void commands.openExternalLink("report_issue").catch(() => undefined)}><ExternalLink aria-hidden="true" size={17} />{text("Report issue", "이슈 제보")}</Button><Button onClick={() => void commands.openExternalLink("releases").catch(() => undefined)}>{text("Releases", "릴리스")}</Button><Button onClick={() => void commands.openExternalLink("kuro_games_official").catch(() => undefined)}>{text("Kuro Games official", "Kuro Games 공식")}</Button></div></section>
  </div>;
}
