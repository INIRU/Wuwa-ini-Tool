import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { IniPreview } from "../api/commands";
import { DiffView } from "./DiffView";
import { AppStateProvider } from "../state/AppState";
import { createFakeCommands } from "../api/commands";

it("keeps full-file diffs in unified mode when semantic changes are unavailable", () => {
  const preview: IniPreview = { token: "token", before_bytes: 4, after_bytes: 8, candidate_text: "[A]\nx=1", diff: [{ kind: "added", old_line: null, new_line: 2, text: "x=1" }], semantic_changes: [], before_encoding: "utf-8", after_encoding: "utf-16le", before_line_endings: "lf", after_line_endings: "crlf", byte_only_change: false };
  render(<AppStateProvider commands={createFakeCommands()}><DiffView preview={preview} /></AppStateProvider>);
  expect(screen.getByRole("button", { name: "Split" })).toBeDisabled();
  expect(screen.getByText(/unified-only full-file diff/i)).toBeVisible();
  expect(screen.getByText(/utf-8 → utf-16le/i)).toBeVisible();
});
