import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { UpdatePrompt } from "./UpdatePrompt";
import type { UpdateInfo, UpdateProgress, UpdateService } from "./update-service";

const available: UpdateInfo = {
  version: "1.0.1",
  notes: "Fixes shader-cache cleanup receipts.",
  published_at: "2026-07-15T02:00:00Z",
};

function updater(overrides: Partial<UpdateService> = {}): UpdateService {
  return {
    check: vi.fn(async () => available),
    downloadAndInstall: vi.fn(async () => undefined),
    defer: vi.fn(),
    isDeferred: vi.fn(() => false),
    ...overrides,
  };
}

function renderPrompt(service: UpdateService, props: Partial<ComponentProps<typeof UpdatePrompt>> = {}) {
  return render(
    <UpdatePrompt
      gameRunning={false}
      maintenanceInProgress={false}
      updater={service}
      writeInProgress={false}
      {...props}
    />,
  );
}

describe("UpdatePrompt", () => {
  beforeEach(() => localStorage.clear());
  afterEach(cleanup);

  it("never installs until the user approves the discovered version", async () => {
    const service = updater();
    const user = userEvent.setup();
    renderPrompt(service);

    expect(await screen.findByText("1.0.1")).toBeInTheDocument();
    expect(screen.getByText("Fixes shader-cache cleanup receipts.")).toBeInTheDocument();
    expect(service.downloadAndInstall).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Update now" }));

    expect(service.downloadAndInstall).toHaveBeenCalledOnce();
  });

  it("reports endpoint failure without blocking the rest of the app", async () => {
    const service = updater({ check: vi.fn(async () => { throw new Error("endpoint_unavailable"); }) });
    render(
      <main>
        <span>Configuration remains available</span>
        <UpdatePrompt gameRunning={false} maintenanceInProgress={false} updater={service} writeInProgress={false} />
      </main>,
    );

    expect(await screen.findByText("Update check unavailable")).toBeInTheDocument();
    expect(screen.getByText("Configuration remains available")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("does not offer an event-announced version when no backend update is pending", async () => {
    const service = updater({ check: vi.fn(async () => null) });
    renderPrompt(service, { announcedVersion: "1.0.1" });

    await waitFor(() => expect(screen.queryByText("Checking for updates…")).not.toBeInTheDocument());
    expect(screen.queryByRole("button", { name: "Update now" })).not.toBeInTheDocument();
  });

  it.each([
    ["a configuration write is active", { writeInProgress: true }],
    ["maintenance is active", { maintenanceInProgress: true }],
    ["Wuthering Waves is running", { gameRunning: true }],
  ])("disables installation while %s", async (_label, blockedProps) => {
    renderPrompt(updater(), blockedProps);

    const button = await screen.findByRole("button", { name: "Update now" });
    expect(button).toBeDisabled();
  });

  it("shows installation and restart progress", async () => {
    let finish: (() => void) | undefined;
    const install = vi.fn((onProgress?: (progress: UpdateProgress) => void) => new Promise<void>((resolve) => {
      onProgress?.({ phase: "installing" });
      finish = () => {
        onProgress?.({ phase: "restarting" });
        resolve();
      };
    }));
    const user = userEvent.setup();
    renderPrompt(updater({ downloadAndInstall: install }));
    await screen.findByText("1.0.1");

    await user.click(screen.getByRole("button", { name: "Update now" }));
    expect(screen.getByText("Installing update…")).toBeInTheDocument();

    finish?.();
    await waitFor(() => expect(screen.getByText("Restarting the app…")).toBeInTheDocument());
  });

  it("renders real backend download progress", async () => {
    const service = updater({ downloadAndInstall: vi.fn(async (onProgress) => {
      onProgress?.({ phase: "downloading", downloaded: "25", total: "100" });
    }) });
    const user = userEvent.setup();
    renderPrompt(service);
    await screen.findByText("1.0.1");

    await user.click(screen.getByRole("button", { name: "Update now" }));

    expect(await screen.findByText("Downloading update… 25%")).toBeVisible();
    expect(screen.getByRole("progressbar", { name: "Downloading update…" })).toHaveValue(25);
  });

  it("reports an install failure without blocking future retries", async () => {
    const service = updater({
      downloadAndInstall: vi.fn(async () => { throw new Error("update_install_failed"); }),
    });
    const user = userEvent.setup();
    renderPrompt(service);
    await screen.findByText("1.0.1");

    await user.click(screen.getByRole("button", { name: "Update now" }));

    expect(await screen.findByText("Update could not be installed")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Update now" })).toBeEnabled();
  });

  it("defers the exact version for the session", async () => {
    const service = updater();
    const onDefer = vi.fn();
    const user = userEvent.setup();
    renderPrompt(service, { onDefer });
    await screen.findByText("1.0.1");

    await user.click(screen.getByRole("button", { name: "Later" }));

    expect(service.defer).toHaveBeenCalledExactlyOnceWith("1.0.1");
    expect(onDefer).toHaveBeenCalledOnce();
    expect(screen.queryByText("1.0.1")).not.toBeInTheDocument();
  });

  it("renders Korean release copy when selected", async () => {
    renderPrompt(updater(), { language: "ko" });

    expect(await screen.findByRole("heading", { name: "업데이트 사용 가능" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "지금 업데이트" })).toBeInTheDocument();
  });
});
