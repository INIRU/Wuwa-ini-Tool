import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { Accordion } from "./Accordion";

afterEach(cleanup);

describe("Accordion", () => {
  it("opens the advanced editor accordion from the keyboard", async () => {
    const user = userEvent.setup();
    render(<Accordion title="Advanced editor">editor</Accordion>);

    expect(screen.getByText("editor")).not.toBeVisible();
    await user.tab();
    await user.keyboard("{Enter}");

    expect(
      screen.getByRole("button", { name: "Advanced editor" }),
    ).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("editor")).toBeVisible();
  });

  it("toggles with Space and connects its button to the panel", async () => {
    const user = userEvent.setup();
    render(<Accordion title="Advanced editor">editor</Accordion>);

    const button = screen.getByRole("button", { name: "Advanced editor" });
    const panelId = button.getAttribute("aria-controls");
    expect(panelId).toBeTruthy();
    expect(document.getElementById(panelId!)).toHaveAttribute("role", "region");

    await user.tab();
    await user.keyboard(" ");
    expect(screen.getByText("editor")).toBeVisible();
    await user.keyboard(" ");
    expect(screen.getByText("editor")).not.toBeVisible();
  });
});
