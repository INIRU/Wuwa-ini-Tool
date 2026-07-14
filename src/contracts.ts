export type AppLanguage = "en" | "ko";

export type ThemeMode = "system" | "light" | "dark";

export type PriorityClass =
  "idle" | "belowNormal" | "normal" | "aboveNormal" | "high" | "realtime";

export type CpuSelection =
  | { mode: "all" }
  | { mode: "performanceCores" }
  | { mode: "cpuSets"; ids: number[] }
  | { mode: "hardAffinity"; group: number; mask: string };

export type GameStatus =
  "notConfigured" | "notRunning" | "launching" | "running" | "exited";

export type CommandError = {
  code: string;
  message: string;
  details?: Record<string, unknown>;
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function parseCommandError(value: unknown): CommandError {
  if (
    !isRecord(value) ||
    typeof value.code !== "string" ||
    typeof value.message !== "string" ||
    (value.details !== undefined && !isRecord(value.details))
  ) {
    throw new TypeError("Invalid command error");
  }

  return {
    code: value.code,
    message: value.message,
    ...(value.details === undefined ? {} : { details: value.details }),
  };
}
