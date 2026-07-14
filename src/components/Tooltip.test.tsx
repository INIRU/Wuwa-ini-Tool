import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { Tooltip } from "./Tooltip";

afterEach(cleanup);

describe("Tooltip", () => {
  it("opens a warning tooltip on focus", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip label="Can make Windows unresponsive">
        <button>Warning</button>
      </Tooltip>,
    );

    await user.tab();

    const trigger = screen.getByRole("button", { name: "Warning" });
    const tooltip = screen.getByRole("tooltip");
    expect(tooltip).toHaveTextContent("Can make Windows unresponsive");
    expect(trigger).toHaveAttribute("aria-describedby", tooltip.id);
  });

  it("opens on hover and closes after the pointer leaves", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip label="Can make Windows unresponsive">
        <button>Warning</button>
      </Tooltip>,
    );

    const trigger = screen.getByRole("button", { name: "Warning" });
    await user.hover(trigger);
    expect(screen.getByRole("tooltip")).toBeVisible();
    await user.unhover(trigger);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("opens on click and closes on Escape", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip label="Can make Windows unresponsive">
        <button>Warning</button>
      </Tooltip>,
    );

    const trigger = screen.getByRole("button", { name: "Warning" });
    await user.click(trigger);
    expect(screen.getByRole("tooltip")).toBeVisible();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });
});
