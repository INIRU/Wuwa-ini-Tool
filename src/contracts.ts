export type AppLanguage = "en" | "ko";
export type ThemeMode = "system" | "light" | "dark";

export type ClientError = { code: string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Parses the deliberately minimal error shape exposed by Rust commands. */
export function parseCommandError(value: unknown): ClientError {
  if (!isRecord(value) || Object.keys(value).length !== 1 || typeof value.code !== "string") {
    throw new TypeError("Invalid command error");
  }
  return { code: value.code };
}
