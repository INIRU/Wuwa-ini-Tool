import { describe, expect, it } from "vitest";
// @ts-expect-error Vitest runs in Node, while the application tsconfig intentionally excludes Node types.
import { readFileSync } from "node:fs";

declare const process: { cwd: () => string };

const tokens = readFileSync(`${process.cwd()}/src/styles/tokens.css`, "utf8");

function relativeLuminance(hex: string): number {
  const channels = hex
    .slice(1)
    .match(/.{2}/g)!
    .map((channel) => Number.parseInt(channel, 16) / 255)
    .map((channel) =>
      channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4,
    );

  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

function contrastRatio(first: string, second: string): number {
  const luminances = [relativeLuminance(first), relativeLuminance(second)].sort(
    (a, b) => b - a,
  );
  return (luminances[0] + 0.05) / (luminances[1] + 0.05);
}

describe("light theme tokens", () => {
  it("keeps primary button text contrast at or above 4.5:1", () => {
    const accent = tokens.match(/--color-accent:\s*(#[0-9a-f]{6})/i)?.[1];
    const foreground = tokens.match(
      /--color-accent-contrast:\s*(#[0-9a-f]{6})/i,
    )?.[1];

    expect(accent).toBeDefined();
    expect(foreground).toBeDefined();
    expect(contrastRatio(accent!, foreground!)).toBeGreaterThanOrEqual(4.5);
  });
});
