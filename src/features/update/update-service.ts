export interface UpdateInfo {
  version: string;
  notes: string | null;
  published_at: string | null;
}

export type UpdateProgress =
  | { phase: "installing" }
  | { phase: "downloading"; downloaded: string; total: string | null }
  | { phase: "restarting" };

export interface UpdateDownloadProgress {
  downloaded: string;
  total: string | null;
}

export interface UpdateBackend {
  getPendingUpdate(): Promise<UpdateInfo | null>;
  installUpdate(confirmed: boolean, onProgress?: (progress: UpdateDownloadProgress) => void): Promise<void>;
}

export interface UpdateService {
  check(): Promise<UpdateInfo | null>;
  downloadAndInstall(onProgress?: (progress: UpdateProgress) => void): Promise<void>;
  defer(version: string): void;
  isDeferred(version: string): boolean;
}

/** Keeps release discovery and installation on the same backend-owned update handle. */
export function createUpdateService(backend: UpdateBackend): UpdateService {
  let pending: UpdateInfo | null = null;
  const deferredVersions = new Set<string>();

  return {
    check: async () => {
      pending = await backend.getPendingUpdate();
      return pending;
    },
    downloadAndInstall: async (onProgress) => {
      if (!pending) throw new Error("update_not_available");
      onProgress?.({ phase: "installing" });
      await backend.installUpdate(true, (progress) => onProgress?.({ phase: "downloading", ...progress }));
      onProgress?.({ phase: "restarting" });
    },
    defer: (version) => {
      deferredVersions.add(version);
    },
    isDeferred: (version) => deferredVersions.has(version),
  };
}
