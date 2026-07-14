import { describe, expect, it, vi } from "vitest";
import { commandErrorMessage, createTauriCommands, normalizeCommandError } from "./commands";

describe("Tauri command adapter", () => {
  it("maps typed inputs to exact command names and camelCase outer arguments", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    const commands = createTauriCommands(invoke);

    await commands.selectGame("candidate-token", true);
    await commands.previewRestoreBackup("backup-id");
    await commands.applyProcessSettings({
      cpu_selection: { mode: "manual_cpu_sets", ids: [2, 3] },
      priority: "above_normal",
      dangerous_priority_acknowledged: false,
    });
    const candidate = { pid: 44, creation_time_100ns: "18446744073709551615", canonical_image: "C:\\Apps\\Worker.exe" };
    await commands.setFocusExclusion("focus-token", candidate, true);
    await commands.normalizeGameQos(true);

    expect(invoke).toHaveBeenNthCalledWith(1, "select_game", {
      candidateToken: "candidate-token",
      confirmed: true,
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "preview_restore_backup", {
      backupId: "backup-id",
    });
    expect(invoke).toHaveBeenNthCalledWith(3, "apply_process_settings", {
      request: {
        cpu_selection: { mode: "manual_cpu_sets", ids: [2, 3] },
        priority: "above_normal",
        dangerous_priority_acknowledged: false,
      },
    });
    expect(invoke).toHaveBeenNthCalledWith(4, "set_focus_exclusion", {
      request: { token: "focus-token", candidate, excluded: true },
    });
    expect(invoke).toHaveBeenNthCalledWith(5, "normalize_game_qos", {
      request: { disable_execution_speed_throttling: true, confirmed: true },
    });
  });

  it("preserves full unsigned 64-bit affinity masks as decimal strings", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);
    const commands = createTauriCommands(invoke);
    await commands.applyProcessSettings({
      cpu_selection: { mode: "hard_affinity", group: 1, mask: "18446744073709551615" },
      priority: "normal",
      dangerous_priority_acknowledged: false,
    });
    expect(invoke).toHaveBeenCalledWith("apply_process_settings", {
      request: expect.objectContaining({ cpu_selection: { mode: "hard_affinity", group: 1, mask: "18446744073709551615" } }),
    });
  });

  it.each([
    [{ code: "game_is_running" }, "game_is_running"],
    ["stale_focus_preview", "stale_focus_preview"],
    [new Error("secret backend detail"), "unexpected_error"],
  ])("normalizes rejected values without exposing arbitrary text", (input, code) => {
    expect(normalizeCommandError(input).code).toBe(code);
  });

  it("maps safety-relevant codes to localized user messages", () => {
    expect(commandErrorMessage({ code: "game_is_running" }, "en")).toMatch(/game is running/i);
    expect(commandErrorMessage({ code: "external_change_detected" }, "ko")).toMatch(/외부/);
    expect(commandErrorMessage({ code: "access_denied" }, "en")).toMatch(/denied/i);
  });

  it("normalizes invoke rejections at the adapter boundary", async () => {
    const commands = createTauriCommands(vi.fn().mockRejectedValue({ code: "stale_focus_preview", detail: "ignored" }));
    await expect(commands.previewFocusMode()).rejects.toMatchObject({ code: "stale_focus_preview" });
  });
});
