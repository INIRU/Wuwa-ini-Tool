import { describe, expect, it, vi } from "vitest";
import { createUpdateService, type UpdateBackend, type UpdateInfo } from "./update-service";

const available: UpdateInfo = {
  version: "1.0.1",
  notes: "Fixes shader-cache cleanup receipts.",
  published_at: "2026-07-15T02:00:00Z",
};

function backend(overrides: Partial<UpdateBackend> = {}): UpdateBackend {
  return {
    getPendingUpdate: vi.fn(async () => available),
    installUpdate: vi.fn(async () => undefined),
    ...overrides,
  };
}

describe("createUpdateService", () => {
  it("checks the backend-owned pending update", async () => {
    const adapter = backend();
    const service = createUpdateService(adapter);

    await expect(service.check()).resolves.toEqual(available);
    expect(adapter.getPendingUpdate).toHaveBeenCalledOnce();
  });

  it("requires discovery before installation", async () => {
    const adapter = backend();
    const service = createUpdateService(adapter);

    await expect(service.downloadAndInstall()).rejects.toThrow("update_not_available");
    expect(adapter.installUpdate).not.toHaveBeenCalled();
  });

  it("passes explicit confirmation through the guarded backend command", async () => {
    const adapter = backend();
    const service = createUpdateService(adapter);
    const phases: string[] = [];
    await service.check();

    await service.downloadAndInstall((progress) => phases.push(progress.phase));

    expect(adapter.installUpdate).toHaveBeenCalledExactlyOnceWith(true, expect.any(Function));
    expect(phases).toEqual(["installing", "restarting"]);
  });

  it("forwards backend-owned byte progress without converting u64 counters", async () => {
    const installUpdate = vi.fn(async (_confirmed: boolean, onProgress?: (progress: { downloaded: string; total: string | null }) => void) => {
      onProgress?.({ downloaded: "18446744073709551614", total: "18446744073709551615" });
    });
    const service = createUpdateService(backend({ installUpdate }));
    const progress: string[] = [];
    await service.check();

    await service.downloadAndInstall((value) => progress.push(value.phase === "downloading" ? `${value.downloaded}/${value.total}` : value.phase));

    expect(progress).toEqual(["installing", "18446744073709551614/18446744073709551615", "restarting"]);
  });

  it("keeps a deferred version hidden for the service lifetime", () => {
    const service = createUpdateService(backend());

    service.defer("1.0.1");

    expect(service.isDeferred("1.0.1")).toBe(true);
    expect(service.isDeferred("1.0.2")).toBe(false);
  });

  it("surfaces a nonblocking check failure to the caller", async () => {
    const service = createUpdateService(backend({
      getPendingUpdate: vi.fn(async () => { throw new Error("endpoint_unavailable"); }),
    }));

    await expect(service.check()).rejects.toThrow("endpoint_unavailable");
  });
});
