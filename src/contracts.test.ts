import { describe, expect, it } from "vitest";
import { parseCommandError } from "./contracts";

describe("parseCommandError", () => {
  it("accepts the code-only ClientError shape", () => {
    expect(parseCommandError({ code: "access_denied" })).toEqual({ code: "access_denied" });
  });

  it.each([null, "access_denied", {}, { code: 403 }, { code: "ok", message: "extra" }])(
    "rejects an unknown error shape: %j",
    (value) => expect(() => parseCommandError(value)).toThrow("Invalid command error"),
  );
});
