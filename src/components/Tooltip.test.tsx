import { cleanup, fireEvent, render, screen } from "@testing-library/react";
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

  it("closes a hover-only tooltip on document Escape", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip label="Can make Windows unresponsive">
        <button>Warning</button>
      </Tooltip>,
    );

    const trigger = screen.getByRole("button", { name: "Warning" });
    await user.hover(trigger);
    expect(trigger).not.toHaveFocus();
    expect(screen.getByRole("tooltip")).toBeVisible();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("keeps the tooltip open for focus after the pointer leaves", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip label="Can make Windows unresponsive">
        <button>Warning</button>
      </Tooltip>,
    );

    const trigger = screen.getByRole("button", { name: "Warning" });
    await user.hover(trigger);
    await user.tab();
    await user.unhover(trigger);

    expect(trigger).toHaveFocus();
    expect(screen.getByRole("tooltip")).toBeVisible();
  });

  it("closes hover, focus, and click reasons independently", () => {
    render(
      <Tooltip label="Can make Windows unresponsive">
        <button>Warning</button>
      </Tooltip>,
    );

    const trigger = screen.getByRole("button", { name: "Warning" });
    fireEvent.mouseEnter(trigger);
    fireEvent.focus(trigger);
    fireEvent.click(trigger);

    fireEvent.mouseLeave(trigger);
    expect(screen.getByRole("tooltip")).toBeVisible();
    fireEvent.blur(trigger);
    expect(screen.getByRole("tooltip")).toBeVisible();
    fireEvent.click(trigger);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("clears all open reasons on document Escape", () => {
    render(
      <Tooltip label="Can make Windows unresponsive">
        <button>Warning</button>
      </Tooltip>,
    );

    const trigger = screen.getByRole("button", { name: "Warning" });
    fireEvent.mouseEnter(trigger);
    fireEvent.focus(trigger);
    fireEvent.click(trigger);
    expect(screen.getByRole("tooltip")).toBeVisible();

    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("preserves an existing accessible description while open", async () => {
    const user = userEvent.setup();
    render(
      <>
        <span id="existing-description">Existing description</span>
        <Tooltip label="Can make Windows unresponsive">
          <button aria-describedby="existing-description">Warning</button>
        </Tooltip>
      </>,
    );

    const trigger = screen.getByRole("button", { name: "Warning" });
    expect(trigger).toHaveAttribute("aria-describedby", "existing-description");

    await user.hover(trigger);
    const tooltip = screen.getByRole("tooltip");
    expect(trigger.getAttribute("aria-describedby")?.split(" ")).toEqual([
      "existing-description",
      tooltip.id,
    ]);

    await user.unhover(trigger);
    expect(trigger).toHaveAttribute("aria-describedby", "existing-description");
  });
});
