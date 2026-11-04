/**
 * The "Formula" popover: a press on a formula opens it, Save or Enter writes
 * only the formula text between its delimiters as one undo step, "Remove
 * formula" takes the element away, a formula that holds its own delimiter
 * is refused with a line that says what to remove, and the Preview shows
 * KaTeX's message for a formula it cannot read.
 */

import { EditorState, StateEffect } from "@codemirror/state";
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import {
  bytes,
  cleanupMarkViews,
  keys,
  setCaret,
  undoOnce,
} from "../../test/markKit";
import { popoverPeers, popoverView } from "../../test/popoverKit";

afterEach(() => {
  cleanup();
  cleanupMarkViews();
});

type Mode = "hidden" | "show";
const MODES: Mode[] = ["hidden", "show"];
const CASES = MODES.flatMap((mode) =>
  [false, true].map((room) => ({ mode, room })),
);

function drawn(selector: string): Element {
  const el = document.querySelector(`.cm-content ${selector}`);
  if (!el) throw new Error(`no ${selector} is drawn`);
  return el;
}

function press(el: Element): void {
  el.dispatchEvent(
    new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }),
  );
}

async function dialog(): Promise<HTMLElement> {
  return screen.findByRole("dialog", { name: "Formula" });
}

describe.each(CASES)("a formula ($mode mode, room $room)", ({ mode, room }) => {
  it("edits inline math in a one-line field and writes on Enter", async () => {
    const user = userEvent.setup();
    const view = popoverView("x\na $`x^2`$ b", { mode, room });
    press(drawn(".cm-math-inline"));
    await dialog();
    const field = screen.getByRole("textbox", { name: "LaTeX" });
    expect(field.tagName).toBe("INPUT");
    expect(field).toHaveValue("x^2");
    expect(field).toHaveFocus();
    await user.clear(field);
    await user.type(field, "y{Enter}");
    expect(bytes(view.state)).toBe("x\na $`y`$ b");
    expect(document.activeElement).toBe(view.contentDOM);
    undoOnce(view);
    expect(bytes(view.state)).toBe("x\na $`x^2`$ b");
  });

  it("edits a dollar block in a textarea and writes on Save", async () => {
    const user = userEvent.setup();
    const view = popoverView("x\n\n$$\na\n$$", { mode, room });
    press(drawn(".cm-math-block"));
    await dialog();
    const field = screen.getByRole("textbox", { name: "LaTeX" });
    expect(field.tagName).toBe("TEXTAREA");
    await user.clear(field);
    await user.type(field, "b{Enter}c");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(bytes(view.state)).toBe("x\n\n$$\nb\nc\n$$");
    undoOnce(view);
    expect(bytes(view.state)).toBe("x\n\n$$\na\n$$");
  });

  it("keeps a math fence's lines as written", async () => {
    const user = userEvent.setup();
    const view = popoverView("x\n\n```math \na\n```", { mode, room });
    press(drawn(".cm-math-block"));
    await dialog();
    const field = screen.getByRole("textbox", { name: "LaTeX" });
    await user.clear(field);
    await user.type(field, "b");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(bytes(view.state)).toBe("x\n\n```math \nb\n```");
  });

  it("removes the formula as one undo step", async () => {
    const user = userEvent.setup();
    const view = popoverView("x\na $`x`$ b", { mode, room });
    press(drawn(".cm-math-inline"));
    await dialog();
    await user.click(screen.getByRole("button", { name: "Remove formula" }));
    expect(bytes(view.state)).toBe("x\na  b");
    undoOnce(view);
    expect(bytes(view.state)).toBe("x\na $`x`$ b");
  });

  it("writes a formula that holds its delimiter so it reads back the same", async () => {
    const user = userEvent.setup();
    const view = popoverView("x\na $`x`$ b", { mode, room });
    press(drawn(".cm-math-inline"));
    await dialog();
    const field = screen.getByRole("textbox", { name: "LaTeX" });
    await user.type(field, "`");
    expect(field).not.toHaveAttribute("aria-describedby");
    await user.keyboard("{Enter}");
    expect(bytes(view.state)).toBe("x\na $`` x` ``$ b");
    undoOnce(view);
    expect(bytes(view.state)).toBe("x\na $`x`$ b");
  });
});

describe("the Preview", () => {
  it("sets the formula with KaTeX", async () => {
    popoverView("x\na $`x^2`$ b");
    press(drawn(".cm-math-inline"));
    await dialog();
    const preview = screen.getByRole("region", { name: "Preview" });
    await waitFor(() => {
      expect(preview.querySelector(".katex")).not.toBeNull();
    });
  });

  it("shows KaTeX's message for a formula it cannot read, and never throws", async () => {
    const user = userEvent.setup();
    popoverView("x\na $`x`$ b");
    press(drawn(".cm-math-inline"));
    await dialog();
    const field = screen.getByRole("textbox", { name: "LaTeX" });
    await user.clear(field);
    await user.type(field, "\\frac{{");
    const preview = screen.getByRole("region", { name: "Preview" });
    expect(
      await within(preview).findByText(/KaTeX parse error/),
    ).toBeInTheDocument();
  });

  it.each([
    [
      "a dollar block",
      "x\n\n$$\na\n$$",
      "a{Enter}$$",
      "Remove the line that holds only $$.",
    ],
    [
      "a math fence",
      "x\n\n```math\na\n```",
      "a{Enter}```",
      "Remove the line of backticks.",
    ],
  ])(
    "refuses %s with a line of its delimiter",
    async (_name, text, typed, end) => {
      const user = userEvent.setup();
      const view = popoverView(text);
      press(drawn(".cm-math-block"));
      await dialog();
      const field = screen.getByRole("textbox", { name: "LaTeX" });
      await user.clear(field);
      await user.type(field, typed);
      expect(field).toHaveAccessibleDescription(
        `This formula cannot be saved. ${end}`,
      );
      expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
      expect(bytes(view.state)).toBe(text);
    },
  );
});

describe("a formula and its peers", () => {
  it("writes nothing to the formula next to it when a peer deleted it", async () => {
    const user = userEvent.setup();
    const { local, peer, sync } = popoverPeers("x\n$`a`$ $`b`$");
    setCaret(local, 4);
    keys(local, "Shift-F10");
    await dialog();
    await user.type(screen.getByRole("textbox", { name: "LaTeX" }), "z");
    peer.dispatch({ changes: { from: 2, to: 8 }, userEvent: "delete" });
    sync();
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(bytes(local.state)).toBe("x\n$`b`$");
  });
});

describe("a page that takes no writes (C12)", () => {
  it("opens nothing from a formula", async () => {
    const view = popoverView("x\na $`x`$ b", { adapter: "reflection" });
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    press(drawn(".cm-math-inline"));
    await Promise.resolve();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("closes without writing when the page stops taking writes before Save", async () => {
    const user = userEvent.setup();
    const view = popoverView("x\na $`x`$ b", { adapter: "reflection" });
    press(drawn(".cm-math-inline"));
    await dialog();
    await user.type(screen.getByRole("textbox", { name: "LaTeX" }), "z");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(bytes(view.state)).toBe("x\na $`x`$ b");
  });
});

describe("a formula on a Reflection page", () => {
  it("says what to remove from a block that a ## line would end", async () => {
    const user = userEvent.setup();
    const view = popoverView("x\n\n$$\na\n$$\n", { adapter: "reflection" });
    press(drawn(".cm-math-block"));
    await dialog();
    const field = screen.getByRole("textbox", { name: "LaTeX" });
    await user.type(field, "{Enter}## b");
    expect(field).toHaveAccessibleDescription(
      "This formula cannot be saved. Remove the ## at the start of its lines.",
    );
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(bytes(view.state)).toBe("x\n\n$$\na\n$$\n");
  });
});
