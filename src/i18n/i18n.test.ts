import { describe, expect, it } from "vitest";
import en from "./en.json";
import ko from "./ko.json";

function leafKeys(value: unknown, prefix = ""): string[] {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return [prefix];
  }

  return Object.entries(value).flatMap(([key, child]) =>
    leafKeys(child, prefix ? `${prefix}.${key}` : key),
  );
}

describe("localization resources", () => {
  it("provides every English key in Korean", () => {
    expect(leafKeys(ko).sort()).toEqual(leafKeys(en).sort());
  });

  it("includes the shared application and safety copy", () => {
    expect(en.app.name).toBe("Wuwa ini Tool");
    expect(ko.app.name).toBe("Wuwa ini Tool");
    expect(en.warning.realtime).toBeTruthy();
    expect(ko.warning.realtime).toBeTruthy();
  });
});
