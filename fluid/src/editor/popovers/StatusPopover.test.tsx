/**
 * The "Status label" popover: a press on a pill opens it, Save or Enter
 * writes only the fields the person changed (the words, the colour, or both)
 * as one undo step, and Escape writes nothing.
 */

import { EditorState, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { cleanup, screen, waitFor } from "@testing-library/react";
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

function pill(): Element {
  const el = document.querySelector(".cm-content .status-label");
  if (!el) throw new Error("no pill is drawn");
  return el;
}

function press(el: Element): MouseEvent {
  const event = new MouseEvent("mousedown", {
    bubbles: true,
    cancelable: true,
    button: 0,
  });
  el.dispatchEvent(event);
  return event;
}

async function dialog(): Promise<HTMLElement> {
  return screen.findByRole("dialog", { name: "Status label" });
}

describe.each(CASES)(
  "a status label ($mode mode, room $room)",
  ({ mode, room }) => {
    it("opens from a press on the pill with the words in Label", async () => {
      popoverView("x\n:status[Done]{green}", { mode, room });
      press(pill());
      await dialog();
      const field = screen.getByRole("textbox", { name: "Label" });
      expect(field).toHaveValue("Done");
      expect(field).toHaveFocus();
      const colours = screen.getByRole("radiogroup", { name: "Colour" });
      expect(colours).toBeInTheDocument();
      expect(screen.getByRole("radio", { name: "green" })).toBeChecked();
      expect(
        screen.getAllByRole("radio").map((r) => r.getAttribute("value")),
      ).toEqual(["grey", "blue", "green", "yellow", "red", "purple"]);
    });

    it("writes new words on Enter and nothing else, as one undo step", async () => {
      const user = userEvent.setup();
      const view = popoverView("x\n:status[Done]{green}", { mode, room });
      press(pill());
      await dialog();
      const field = screen.getByRole("textbox", { name: "Label" });
      await user.clear(field);
      await user.type(field, "Shipped{Enter}");
      expect(bytes(view.state)).toBe("x\n:status[Shipped]{green}");
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(document.activeElement).toBe(view.contentDOM);
      undoOnce(view);
      expect(bytes(view.state)).toBe("x\n:status[Done]{green}");
    });

    it("writes a new colour on Save", async () => {
      const user = userEvent.setup();
      const view = popoverView("x\n:status[Done]{green}", { mode, room });
      press(pill());
      await dialog();
      await user.click(screen.getByRole("radio", { name: "blue" }));
      await user.click(screen.getByRole("button", { name: "Save" }));
      expect(bytes(view.state)).toBe("x\n:status[Done]{blue}");
      undoOnce(view);
      expect(bytes(view.state)).toBe("x\n:status[Done]{green}");
    });

    it("writes both in one undo step", async () => {
      const user = userEvent.setup();
      const view = popoverView("x\n:status[Done]{green}", { mode, room });
      press(pill());
      await dialog();
      await user.click(screen.getByRole("radio", { name: "blue" }));
      const field = screen.getByRole("textbox", { name: "Label" });
      await user.clear(field);
      await user.type(field, "a]b{Enter}");
      expect(bytes(view.state)).toBe("x\n:status[a\\]b]{blue}");
      undoOnce(view);
      expect(bytes(view.state)).toBe("x\n:status[Done]{green}");
    });

    it("does not save empty words", async () => {
      const user = userEvent.setup();
      const view = popoverView("x\n:status[Done]{green}", { mode, room });
      press(pill());
      await dialog();
      await user.clear(screen.getByRole("textbox", { name: "Label" }));
      expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
      await user.keyboard("{Enter}");
      expect(bytes(view.state)).toBe("x\n:status[Done]{green}");
    });

    it("writes nothing on Escape and gives the focus back", async () => {
      const user = userEvent.setup();
      const view = popoverView("x\n:status[Done]{green}", { mode, room });
      press(pill());
      await dialog();
      await user.type(screen.getByRole("textbox", { name: "Label" }), "zz");
      await user.keyboard("{Escape}");
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(bytes(view.state)).toBe("x\n:status[Done]{green}");
      expect(document.activeElement).toBe(view.contentDOM);
    });
  },
);

describe("a peer's change while the label is open", () => {
  it("keeps the peer's colour beside the person's words", async () => {
    const user = userEvent.setup();
    const { local, peer, sync } = popoverPeers("x\n:status[Done]{green}");
    press(pill());
    await dialog();
    peer.dispatch({
      changes: { from: 16, to: 21, insert: "red" },
      userEvent: "input",
    });
    sync();
    const field = screen.getByRole("textbox", { name: "Label" });
    await user.clear(field);
    await user.type(field, "Shipped{Enter}");
    expect(bytes(local.state)).toBe("x\n:status[Shipped]{red}");
  });

  it("keeps the peer's words beside the person's colour", async () => {
    const user = userEvent.setup();
    const { local, peer, sync } = popoverPeers("x\n:status[Done]{green}");
    press(pill());
    await dialog();
    peer.dispatch({
      changes: { from: 10, to: 14, insert: "Later" },
      userEvent: "input",
    });
    sync();
    await user.click(screen.getByRole("radio", { name: "blue" }));
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(bytes(local.state)).toBe("x\n:status[Later]{blue}");
  });

  it("writes nothing to the label next to it when a peer deleted it", async () => {
    const user = userEvent.setup();
    const { local, peer, sync } = popoverPeers("x\n:status[a] :status[b]");
    setCaret(local, 5);
    keys(local, "Shift-F10");
    await dialog();
    await user.type(screen.getByRole("textbox", { name: "Label" }), "z");
    peer.dispatch({ changes: { from: 2, to: 13 }, userEvent: "delete" });
    sync();
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(bytes(local.state)).toBe("x\n:status[b]");
    expect(document.activeElement).toBe(local.contentDOM);
  });
});

describe("a page that takes no writes (C12)", () => {
  it("opens nothing from a pill or Shift+F10", async () => {
    const view = popoverView("x\n:status[Done]{green}", {
      adapter: "reflection",
    });
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    press(pill());
    setCaret(view, 5);
    keys(view, "Shift-F10");
    await Promise.resolve();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("closes without writing when the page stops taking writes before Save", async () => {
    const user = userEvent.setup();
    const view = popoverView("x\n:status[Done]{green}", {
      adapter: "reflection",
    });
    press(pill());
    await dialog();
    await user.type(screen.getByRole("textbox", { name: "Label" }), "z");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorView.editable.of(false)),
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(bytes(view.state)).toBe("x\n:status[Done]{green}");
  });
});
