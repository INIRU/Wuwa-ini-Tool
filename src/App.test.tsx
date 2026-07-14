import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { createFakeCommands, type RuntimeEventHandlers } from "./api/commands";

beforeEach(() => localStorage.clear());
afterEach(cleanup);

async function openApp() {
  const user = userEvent.setup();
  render(<App commands={createFakeCommands()} />);
  expect(await screen.findByRole("dialog", { name: /important notice/i })).toBeVisible();
  await user.click(screen.getByRole("checkbox", { name: /understand/i }));
  await user.click(screen.getByRole("button", { name: /continue/i }));
  expect(await screen.findByRole("heading", { name: /dashboard/i })).toBeVisible();
  return user;
}

describe("App workflows", () => {
  it("requires the bilingual first-run disclaimer before entering the app", async () => {
    const getAppSnapshot = vi.fn(createFakeCommands().getAppSnapshot);
    const listProfiles = vi.fn(createFakeCommands().listProfiles);
    const subscribe = vi.fn(createFakeCommands().subscribe);
    render(<App commands={createFakeCommands({ getAppSnapshot, listProfiles, subscribe })} />);

    const dialog = await screen.findByRole("dialog", { name: /important notice/i });
    expect(screen.queryByRole("main")).not.toBeInTheDocument();
    expect(dialog).toHaveTextContent("게임 충돌");
    expect(dialog).toHaveTextContent("game crashes");
    expect(within(dialog).getByRole("button", { name: /continue/i })).toBeDisabled();
    expect(getAppSnapshot).not.toHaveBeenCalled();
    expect(listProfiles).not.toHaveBeenCalled();
    expect(subscribe).not.toHaveBeenCalled();
  });

  it("previews pasted Engine.ini text and applies only the current token", async () => {
    const applyIni = vi.fn().mockResolvedValue({ applied_sha256: "new-sha", backup_id: "backup-2" });
    const commands = createFakeCommands({ applyIni });
    const user = userEvent.setup();
    render(<App commands={commands} />);
    await user.click(await screen.findByRole("checkbox", { name: /understand/i }));
    await user.click(screen.getByRole("button", { name: /continue/i }));
    await user.click(screen.getByRole("button", { name: "Engine.ini" }));
    await user.click(screen.getByRole("button", { name: /paste a complete Engine.ini/i }));
    expect(screen.getByText(/replaces the complete Engine.ini/i)).toBeVisible();
    await user.type(screen.getByLabelText(/Engine.ini content/i), "[SystemSettings]{enter}r.IniruFPSOpti=1");
    await user.click(screen.getByRole("button", { name: /preview pasted file/i }));

    expect(within(await screen.findByRole("region", { name: /Engine.ini diff/i })).getByText(/r.IniruFPSOpti=1/)).toBeVisible();
    await user.click(screen.getByRole("checkbox", { name: /confirm backup/i }));
    await user.click(screen.getByRole("button", { name: /back up and apply/i }));
    expect(applyIni).toHaveBeenCalledWith("ini-preview-token", true);
    expect(await screen.findByText(/backup-2/)).toBeVisible();
  });

  it("keeps active Focus state across routes and surfaces restore failures", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const deactivateFocusMode = vi.fn().mockRejectedValue({ code: "focus_restore_failed" });
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ deactivateFocusMode })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: /CPU & Priority/i }));
    const focusSwitch = screen.getByRole("checkbox", { name: /Off \/ On/i });
    await user.click(focusSwitch);
    await user.click(screen.getByRole("button", { name: /Preview eligible processes/i }));
    await user.click(screen.getByRole("checkbox", { name: /Select Indexer/i }));
    await user.click(screen.getByRole("button", { name: /Activate from this preview/i }));
    await user.click(screen.getByRole("button", { name: /^Settings$/i }));
    await user.click(screen.getByRole("button", { name: /CPU & Priority/i }));
    const activeSwitch = screen.getByRole("checkbox", { name: /Off \/ On/i });
    expect(activeSwitch).toBeChecked();
    await user.click(activeSwitch);
    expect(activeSwitch).toBeChecked();
    expect(await screen.findByText(/focus_restore_failed/i)).toBeVisible();
  });

  it("keeps Focus restoring state global during route changes", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    let rejectRestore!: (error: unknown) => void;
    const deactivateFocusMode = vi.fn(() => new Promise<never>((_resolve, reject) => { rejectRestore = reject; }));
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ deactivateFocusMode })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: /CPU & Priority/i }));
    await user.click(screen.getByRole("checkbox", { name: /Off \/ On/i }));
    await user.click(screen.getByRole("checkbox", { name: /Off \/ On/i }));
    await user.click(screen.getByRole("button", { name: /^Settings$/i }));
    await user.click(screen.getByRole("button", { name: /CPU & Priority/i }));
    expect(screen.getByRole("checkbox", { name: /Restoring/i })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /Restoring/i })).toBeDisabled();
    await act(async () => { rejectRestore({ code: "focus_restore_failed" }); await Promise.resolve(); });
    expect(await screen.findByText(/focus_restore_failed/i)).toBeVisible();
    expect(screen.getByRole("checkbox", { name: /Off \/ On/i })).toBeChecked();
  });

  it("resets installation confirmation when a new candidate token is selected", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const first = await createFakeCommands().discoverGameManual();
    const discoverGameManual = vi.fn().mockResolvedValueOnce(first).mockResolvedValueOnce(first ? { ...first, token: "candidate-token-2" } : null);
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ discoverGameManual })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: /Settings/i }));
    await user.click(screen.getByRole("button", { name: /Choose game executable/i }));
    await user.click(await screen.findByRole("checkbox", { name: /Confirm this validated installation/i }));
    await user.click(screen.getByRole("button", { name: /Choose game executable/i }));
    expect(await screen.findByRole("checkbox", { name: /Confirm this validated installation/i })).not.toBeChecked();
  });

  it("discards a late cleanup preview after its selection changes", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const basePreview = await createFakeCommands().previewCacheCleanup({ wuwa: true, nvidia: false });
    let resolveCleanup!: (value: typeof basePreview) => void;
    const previewCacheCleanup = vi.fn(() => new Promise<typeof basePreview>((resolve) => { resolveCleanup = resolve; }));
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ previewCacheCleanup })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: /Settings/i }));
    const wuwa = screen.getByRole("checkbox", { name: /WuWa shader cache/i });
    await user.click(wuwa);
    await user.click(screen.getByRole("button", { name: /Preview cleanup/i }));
    await user.click(wuwa);
    await act(async () => resolveCleanup(basePreview));
    expect(screen.queryByRole("heading", { name: /Cleanup preview/i })).not.toBeInTheDocument();
  });

  it("discards a late profile preview after another profile is selected", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const base = (await createFakeCommands().listProfiles())[0];
    const profiles = [
      { ...base, id: "profile-a", name: "Profile A", patch: { ...base.patch, managed_ini: [{ section: "SystemSettings", key: "r.A", value: "1" }] } },
      { ...base, id: "profile-b", name: "Profile B", patch: { ...base.patch, managed_ini: [{ section: "SystemSettings", key: "r.B", value: "1" }] } },
    ];
    const previewA = await createFakeCommands().previewIni({ kind: "paste", text: "[SystemSettings]\nr.A=1" });
    const previewB = await createFakeCommands().previewIni({ kind: "paste", text: "[SystemSettings]\nr.B=1" });
    let resolveA!: (value: typeof previewA) => void;
    const previewIni = vi.fn()
      .mockImplementationOnce(() => new Promise<typeof previewA>((resolve) => { resolveA = resolve; }))
      .mockResolvedValueOnce(previewB);
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ listProfiles: async () => profiles, previewIni })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: "Profiles" }));
    const previewButtons = await screen.findAllByRole("button", { name: "Preview" });
    await user.click(previewButtons[0]);
    await user.click(previewButtons[1]);
    expect(await screen.findByText(/r.B=1/)).toBeVisible();
    await act(async () => resolveA(previewA));
    expect(screen.getByText(/r.B=1/)).toBeVisible();
    expect(screen.queryByText(/r.A=1/)).not.toBeInTheDocument();
  });

  it("renders technical outcomes with Korean surface labels", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    localStorage.setItem("wuwa-language", "ko");
    const baseSnapshot = await createFakeCommands().getAppSnapshot();
    const focusPreview = await createFakeCommands().previewFocusMode();
    const activeProcess = { pid: 101, creation_time_100ns: "18446744073709551615", canonical_image: "C:\\Wuthering Waves.exe" };
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({
      getAppSnapshot: async () => ({ ...baseSnapshot, active_process: activeProcess, supervisor_state: "active" }),
      activateFocusMode: async () => ({ epoch: 3, process: activeProcess, status: "applied", process_results: [{ identity: focusPreview.candidates[0].identity, outcome: "selected" }], recovery_required: false, telemetry: null, adaptive_decision: { contention: "aggregate", action: "restrain_priority", priority_target_count: 1, background_cpu_set_ids: [], game_cpu_selection: { mode: "all" } } }),
    })} />);
    await screen.findByRole("heading", { name: "대시보드" });
    await user.click(screen.getByRole("button", { name: "CPU 및 우선도" }));
    await user.click(screen.getByRole("button", { name: "프로세스 설정 적용" }));
    expect(await screen.findByText(/결과: 성공/)).toHaveTextContent("우선도 보통");
    await user.click(screen.getByRole("checkbox", { name: /실행 속도 제한 해제/ }));
    await user.click(screen.getByRole("button", { name: "게임 QoS 정규화" }));
    expect(await screen.findByText(/QoS 적용됨/)).toBeVisible();
    await user.click(screen.getByRole("checkbox", { name: /꺼짐 \/ 켜짐/ }));
    await user.click(screen.getByRole("button", { name: "대상 프로세스 미리보기" }));
    expect(screen.getByText("샘플 간격")).toBeVisible();
    await user.click(screen.getByRole("checkbox", { name: "Indexer 선택" }));
    await user.click(screen.getByRole("button", { name: "이 미리보기로 활성화" }));
    expect(await screen.findByText(/전체 경합/)).toHaveTextContent("우선도 제한");
    expect(screen.getByText(/선택됨/)).toBeVisible();
    await user.click(screen.getByRole("button", { name: "설정" }));
    await user.click(screen.getByRole("checkbox", { name: /NVIDIA 드라이버 캐시/ }));
    await user.click(screen.getByRole("button", { name: "정리 미리보기" }));
    expect(await screen.findByText(/파일 ·/)).toBeVisible();
    expect(screen.getByText(/건너뜀/)).toBeVisible();
  });

  it("shows a discovered update globally without opening Settings", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    render(<App commands={createFakeCommands({ getPendingUpdate: async () => ({ version: "1.0.1", notes: "Signed fixes", published_at: "2026-07-15T02:00:00Z" }) })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    expect(await screen.findByRole("heading", { name: "Update available" })).toBeVisible();
    expect(screen.getByText("Signed fixes")).toBeVisible();
  });

  it("adds uncatalogued custom entries to a managed preview", async () => {
    const previewIni = vi.fn(createFakeCommands().previewIni);
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ previewIni })} />);
    await user.click(await screen.findByRole("checkbox", { name: /understand/i }));
    await user.click(screen.getByRole("button", { name: /continue/i }));
    await user.click(screen.getByRole("button", { name: "Engine.ini" }));
    await user.click(screen.getByRole("button", { name: /custom option/i }));
    await user.type(screen.getByLabelText(/^Section$/i), "SystemSettings");
    await user.type(screen.getByLabelText(/^Key$/i), "r.IniruFPSOpti");
    await user.type(screen.getByLabelText(/^Value$/i), "1");
    await user.click(screen.getByRole("button", { name: /add option/i }));
    expect(screen.getByText("r.IniruFPSOpti=1")).toBeVisible();
    expect(screen.getByText(/not runtime-verified/i)).toBeVisible();
    await user.click(screen.getByRole("button", { name: /preview managed changes/i }));
    expect(previewIni).toHaveBeenCalledWith({
      kind: "managed",
      changes: [{ section: "SystemSettings", key: "r.IniruFPSOpti", value: "1" }],
    });
  });

  it("shows elevated priority warnings and sends serde-compatible values", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const applyProcessSettings = vi.fn(createFakeCommands().applyProcessSettings);
    const getAppSnapshot = vi.fn(async () => ({ ...(await createFakeCommands().getAppSnapshot()), active_process: { pid: 101, creation_time_100ns: "44", canonical_image: "C:\\Wuthering Waves.exe" }, supervisor_state: "active" as const }));
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ applyProcessSettings, getAppSnapshot })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: /CPU & Priority/i }));
    await user.selectOptions(screen.getByLabelText(/priority class/i), "high");
    expect(screen.getByText(/system responsiveness/i)).toBeVisible();
    await user.hover(screen.getByRole("button", { name: /warning for high priority/i }));
    expect(screen.getByRole("tooltip")).toHaveTextContent(/system responsiveness/i);
    await user.click(screen.getByRole("checkbox", { name: /accept the elevated/i }));
    await user.click(screen.getByRole("button", { name: /apply process settings/i }));
    expect(applyProcessSettings).toHaveBeenCalledWith({
      cpu_selection: { mode: "all" },
      priority: "high",
      dangerous_priority_acknowledged: true,
    });
  });

  it("keeps cache cleanup unselected and requires a preview before cleanup", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const user = userEvent.setup();
    render(<App commands={createFakeCommands()} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: /Settings/i }));
    expect(screen.getByRole("checkbox", { name: /WuWa shader cache/i })).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: /NVIDIA driver cache/i })).not.toBeChecked();
    expect(screen.getByRole("button", { name: /preview cleanup/i })).toBeDisabled();
  });

  it("exposes every primary route through the keyboard-friendly left rail", async () => {
    const user = await openApp();
    for (const name of ["Engine.ini", "CPU & Priority", "Profiles", "Backups", "Settings", "About"]) {
      await user.click(screen.getByRole("button", { name }));
      expect(screen.getByRole("main")).toHaveAttribute("data-page");
    }
  });

  it("switches navigation and page headings to Korean", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const user = userEvent.setup();
    render(<App commands={createFakeCommands()} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: "Settings" }));
    await user.selectOptions(screen.getByLabelText("Language"), "ko");
    expect(screen.getByRole("button", { name: "홈" })).toBeVisible();
    await user.click(screen.getByRole("button", { name: "홈" }));
    expect(screen.getByRole("heading", { name: "대시보드" })).toBeVisible();
  });

  it("uses the Windows locale on first launch and applies the selected theme", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    Object.defineProperty(navigator, "language", { configurable: true, value: "ko-KR" });
    const user = userEvent.setup();
    render(<App commands={createFakeCommands()} />);
    expect(await screen.findByRole("button", { name: "홈" })).toBeVisible();
    await user.click(screen.getByRole("button", { name: "설정" }));
    await user.selectOptions(screen.getByLabelText("테마"), "dark");
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
    Object.defineProperty(navigator, "language", { configurable: true, value: "en-US" });
  });

  it("preserves Vanilla preset deletions in the managed preview request", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const previewIni = vi.fn(createFakeCommands().previewIni);
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ previewIni })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: "Engine.ini" }));
    await user.selectOptions(screen.getByLabelText("Preset"), "vanilla");
    await user.click(screen.getByRole("button", { name: /custom option/i }));
    await user.click(screen.getByRole("button", { name: /preview managed changes/i }));
    expect(previewIni).toHaveBeenCalledWith(expect.objectContaining({
      kind: "managed",
      changes: expect.arrayContaining([{ section: "SystemSettings", key: "r.ScreenPercentage", value: null }]),
    }));
  });

  it("previews a saved profile without auto-applying it", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const previewIni = vi.fn(createFakeCommands().previewIni);
    const applyIni = vi.fn(createFakeCommands().applyIni);
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ previewIni, applyIni })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: "Profiles" }));
    await user.click(await screen.findByRole("button", { name: "Preview" }));
    expect(previewIni).toHaveBeenCalledWith({ kind: "managed", changes: [] });
    expect(applyIni).not.toHaveBeenCalled();
    await user.click(screen.getByRole("checkbox", { name: /confirm backup/i }));
    await user.click(screen.getByRole("button", { name: /Back up and apply Engine.ini/i }));
    expect(applyIni).toHaveBeenCalledWith("ini-preview-token", true);
  });

  it("keeps NVIDIA-only cache preview available while the game is running", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const getAppSnapshot = vi.fn(async () => ({ ...(await createFakeCommands().getAppSnapshot()), active_process: { pid: 101, creation_time_100ns: "44", canonical_image: "C:\\Wuthering Waves.exe" }, supervisor_state: "active" as const }));
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ getAppSnapshot })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByRole("checkbox", { name: /WuWa shader cache/i })).toBeDisabled();
    const nvidia = screen.getByRole("checkbox", { name: /NVIDIA driver cache/i });
    expect(nvidia).toBeEnabled();
    await user.click(nvidia);
    expect(screen.getByRole("button", { name: /Preview cleanup/i })).toBeEnabled();
  });

  it("clears confirmation for every new INI preview token", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const user = userEvent.setup();
    render(<App commands={createFakeCommands()} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: "Engine.ini" }));
    await user.click(screen.getByRole("button", { name: /paste a complete/i }));
    await user.type(screen.getByLabelText(/Engine.ini content/i), "[SystemSettings]{enter}r.Test=1");
    await user.click(screen.getByRole("button", { name: /preview pasted file/i }));
    const confirmation = await screen.findByRole("checkbox", { name: /confirm backup/i });
    await user.click(confirmation);
    expect(confirmation).toBeChecked();
    await user.click(screen.getByRole("button", { name: /Import Engine.ini/i }));
    expect(await screen.findByRole("checkbox", { name: /confirm backup/i })).not.toBeChecked();
  });

  it("discards a late INI preview after its input generation changes", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const response = await createFakeCommands().previewIni({ kind: "paste", text: "[SystemSettings]\nr.Late=1" });
    let resolvePreview!: (value: typeof response) => void;
    const previewIni = vi.fn(() => new Promise<typeof response>((resolve) => { resolvePreview = resolve; }));
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ previewIni })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: "Engine.ini" }));
    await user.click(screen.getByRole("button", { name: /paste a complete/i }));
    const editor = screen.getByLabelText(/Engine.ini content/i);
    await user.type(editor, "[SystemSettings]{enter}r.Test=1");
    await user.click(screen.getByRole("button", { name: /preview pasted file/i }));
    await user.type(editor, "{enter}r.Changed=1");
    await act(async () => resolvePreview(response));
    expect(screen.queryByRole("region", { name: /Engine.ini diff/i })).not.toBeInTheDocument();
  });

  it("delivers Focus recovery and runtime errors from subscribed events", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    let handlers: RuntimeEventHandlers | undefined;
    const subscribe = vi.fn(async (next: RuntimeEventHandlers) => { handlers = next; return () => undefined; });
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ subscribe })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: "CPU & Priority" }));
    const report = await createFakeCommands().activateFocusMode({ token: "focus-preview-uuid", selected: [], select_all_eligible: true, select_all_confirmed: true });
    await act(async () => handlers?.onFocusReport?.({ ...report, status: "recovery_required", recovery_required: true }));
    expect(screen.getByText(/Focus recovery required/i)).toBeVisible();
    expect(screen.getByText(/Recovery is required/i)).toBeVisible();
    await act(async () => handlers?.onSupervisorError?.({ code: "focus_restore_failed" }));
    expect(screen.getByText(/focus_restore_failed/i)).toBeVisible();
  });

  it("discards a consumed profile import token after a save collision", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const saveImportedProfile = vi.fn().mockRejectedValue({ code: "profile_id_collision" });
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ saveImportedProfile })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: "Profiles" }));
    await user.click(screen.getByRole("button", { name: /Import profile/i }));
    expect(await screen.findByRole("heading", { name: /Import preview/i })).toBeVisible();
    await user.click(screen.getByRole("button", { name: /Save imported profile/i }));
    expect(screen.queryByRole("heading", { name: /Import preview/i })).not.toBeInTheDocument();
    expect(screen.getByText(/Import the file again/i)).toBeVisible();
  });

  it("discards a late profile import candidate token", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const base = await createFakeCommands().importProfile();
    if (!base) throw new Error("missing fixture");
    let resolveFirst!: (value: typeof base | null) => void;
    const importProfile = vi.fn()
      .mockImplementationOnce(() => new Promise<typeof base | null>((resolve) => { resolveFirst = resolve; }))
      .mockResolvedValueOnce({ ...base, token: "new-token", preview: { ...base.preview, display_name: "Newest Profile" } });
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ importProfile })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: "Profiles" }));
    const importButton = screen.getByRole("button", { name: /Import profile/i });
    await user.click(importButton);
    await user.click(importButton);
    expect(await screen.findByDisplayValue("Newest Profile")).toBeVisible();
    await act(async () => resolveFirst(base));
    expect(screen.getByDisplayValue("Newest Profile")).toBeVisible();
    expect(screen.queryByDisplayValue("Shared Performance")).not.toBeInTheDocument();
  });

  it("keeps discovered installation tokens until explicit path confirmation", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const base = await createFakeCommands().getAppSnapshot();
    const selectGame = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ getAppSnapshot: async () => ({ ...base, installation: null }), selectGame })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: /Discover game/i }));
    const useButton = await screen.findByRole("button", { name: /Use this installation/i });
    expect(useButton).toBeDisabled();
    await user.click(screen.getByRole("checkbox", { name: /Confirm this executable/i }));
    await user.click(useButton);
    expect(selectGame).toHaveBeenCalledWith("candidate-token", true);
  });

  it("invalidates a WuWa cache preview when the game starts", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const base = await createFakeCommands().getAppSnapshot();
    const running = { ...base, active_process: { pid: 101, creation_time_100ns: "44", canonical_image: "C:\\Wuthering Waves.exe" }, supervisor_state: "active" as const };
    const getAppSnapshot = vi.fn().mockResolvedValueOnce(base).mockResolvedValue(running);
    let handlers: RuntimeEventHandlers | undefined;
    const subscribe = vi.fn(async (next: RuntimeEventHandlers) => { handlers = next; return () => undefined; });
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ getAppSnapshot, subscribe })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const wuwa = screen.getByRole("checkbox", { name: /WuWa shader cache/i });
    await user.click(wuwa);
    expect(wuwa).toBeChecked();
    await act(async () => handlers?.onSupervisor?.({ state: "active" }));
    await waitFor(() => expect(wuwa).not.toBeChecked());
    expect(wuwa).toBeDisabled();
  });

  it("saves the actual shared Engine and process draft as a profile", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const saveProfile = vi.fn(createFakeCommands().saveProfile);
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ saveProfile })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: "Engine.ini" }));
    await user.click(screen.getByRole("button", { name: /custom option/i }));
    await user.type(screen.getByLabelText(/^Section$/i), "SystemSettings");
    await user.type(screen.getByLabelText(/^Key$/i), "r.SharedDraft");
    await user.type(screen.getByLabelText(/^Value$/i), "1");
    await user.click(screen.getByRole("button", { name: /add option/i }));
    await user.click(screen.getByRole("button", { name: "CPU & Priority" }));
    await user.selectOptions(screen.getByLabelText(/priority class/i), "above_normal");
    await user.click(screen.getByRole("button", { name: "Profiles" }));
    await user.type(screen.getByLabelText("Profile id"), "shared-draft");
    await user.type(screen.getAllByLabelText(/Display name/i)[0], "Shared Draft");
    await user.click(screen.getByRole("button", { name: /Save current/i }));
    expect(saveProfile).toHaveBeenCalledWith(expect.objectContaining({ patch: expect.objectContaining({ custom_ini_entries: [expect.objectContaining({ key: "r.SharedDraft", value: "1" })], process: { cpu_selection: { mode: "all" }, priority: "above_normal" } }) }));
  });

  it("shows protected Focus fixtures and requires separate select-all confirmation", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const user = userEvent.setup();
    render(<App commands={createFakeCommands()} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: "CPU & Priority" }));
    await user.click(screen.getByRole("checkbox", { name: /Off \/ On/i }));
    await user.click(screen.getByRole("button", { name: /Preview eligible processes/i }));
    for (const [name, reason] of [["Discord", "communication"], ["OBS Studio", "recording"], ["Streamlabs", "streaming"], ["XSplit", "capture overlay"]]) {
      expect(screen.getByText(reason)).toBeVisible();
      expect(screen.getByRole("checkbox", { name: `Select ${name}` })).toBeDisabled();
    }
    await user.click(screen.getByRole("checkbox", { name: /Select all eligible/i }));
    expect(screen.getByText(/newly eligible processes/i)).toBeVisible();
    const activate = screen.getByRole("button", { name: /Activate from this preview/i });
    expect(activate).toBeDisabled();
    await user.click(screen.getByRole("checkbox", { name: /Confirm dynamic select-all/i }));
    expect(activate).toBeEnabled();
  });

  it("pins Focus exclusions and runs explicitly confirmed QoS normalization", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const base = await createFakeCommands().getAppSnapshot();
    const getAppSnapshot = vi.fn(async () => ({ ...base, active_process: { pid: 101, creation_time_100ns: "44", canonical_image: "C:\\Wuthering Waves.exe" }, supervisor_state: "active" as const }));
    const setFocusExclusion = vi.fn(createFakeCommands().setFocusExclusion);
    const normalizeGameQos = vi.fn(createFakeCommands().normalizeGameQos);
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ getAppSnapshot, setFocusExclusion, normalizeGameQos })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: /CPU & Priority/i }));
    await user.click(screen.getByRole("checkbox", { name: /Off \/ On/i }));
    await user.click(screen.getByRole("button", { name: /Preview eligible processes/i }));
    await user.click(screen.getAllByRole("button", { name: /Always exclude/i })[0]);
    expect(setFocusExclusion).toHaveBeenCalledWith("focus-preview-uuid", expect.objectContaining({ canonical_image: "C:\\Apps\\Indexer.exe" }), true);
    const qos = screen.getByRole("checkbox", { name: /Confirm disabling execution-speed throttling/i });
    await user.click(qos);
    await user.click(screen.getByRole("button", { name: /Normalize game QoS/i }));
    expect(normalizeGameQos).toHaveBeenCalledWith(true);
  });

  it("sends the maximum u64 affinity mask losslessly", async () => {
    localStorage.setItem("wuwa-disclaimer-v1", "accepted");
    const base = await createFakeCommands().getAppSnapshot();
    const getAppSnapshot = vi.fn(async () => ({ ...base, active_process: { pid: 101, creation_time_100ns: "44", canonical_image: "C:\\Wuthering Waves.exe" }, supervisor_state: "active" as const }));
    const applyProcessSettings = vi.fn(createFakeCommands().applyProcessSettings);
    const user = userEvent.setup();
    render(<App commands={createFakeCommands({ getAppSnapshot, applyProcessSettings })} />);
    await screen.findByRole("heading", { name: /dashboard/i });
    await user.click(screen.getByRole("button", { name: /CPU & Priority/i }));
    await user.selectOptions(screen.getByLabelText(/CPU policy/i), "hard_affinity");
    const mask = screen.getByLabelText(/Affinity mask/i);
    await user.clear(mask);
    await user.type(mask, "18446744073709551615");
    await user.click(screen.getByRole("button", { name: /Apply process settings/i }));
    expect(applyProcessSettings).toHaveBeenCalledWith(expect.objectContaining({ cpu_selection: { mode: "hard_affinity", group: 0, mask: "18446744073709551615" } }));
  });
});
