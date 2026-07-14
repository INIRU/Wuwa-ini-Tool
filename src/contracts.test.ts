import { describe, expect, it } from "vitest";
import { parseCommandError } from "./contracts";

describe("parseCommandError", () => {
  it("returns a valid command error", () => {
    const error = {
      code: "access_denied",
      message: "The operation was denied.",
      details: { path: "Engine.ini" },
    };

    expect(parseCommandError(error)).toEqual(error);
  });

  it.each([
    null,
    "access_denied",
    { code: "access_denied" },
    { code: 403, message: "The operation was denied." },
    { code: "access_denied", message: false },
    { code: "access_denied", message: "Denied", details: [] },
  ])("rejects an unknown error shape: %j", (value) => {
    expect(() => parseCommandError(value)).toThrow("Invalid command error");
  });
});
