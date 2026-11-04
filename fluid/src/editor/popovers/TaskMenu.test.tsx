/**
 * The "Task state" menu: a right click on a task box or Shift+F10 on a task
 * line opens it, the arrow keys move between its three states, and picking
 * one writes the one character of the box as one undo step.
 */

import { EditorState, StateEffect } from "@codemirror/state";
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

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

function box(): Element {
  const el = document.querySelector(
    ".cm-content .cm-task-toggle, .cm-content .cm-task-na",
  );
  if (!el) throw new Error("no task box is drawn");
  return el;
}

function rightClick(el: Element): MouseEvent {
  const event = new MouseEvent("contextmenu", {
    bubbles: true,
    cancelable: true,
    button: 2,
  });
  el.dispatchEvent(event);
  return event;
}

async function menu(): Promise<HTMLElement> {
  return screen.findByRole("menu", { name: "Task state" });
}

describe.each(CASES)(
  "the task menu ($mode mode, room $room)",
  ({ mode, room }) => {
    it("opens from a right click on the box with the state checked", async () => {
      popoverView("x\n- [ ] t", { mode, room });
      const event = rightClick(box());
      await menu();
      expect(event.defaultPrevented).toBe(true);
      const items = screen.getAllByRole("menuitemradio");
      expect(items.map((item) => item.textContent)).toEqual([
        "Open",
        "Done",
        "Not applicable",
      ]);
      const open = screen.getByRole("menuitemradio", { name: "Open" });
      expect(open).toHaveAttribute("aria-checked", "true");
      expect(open).toHaveFocus();
    });

    it("writes Not applicable as one undo step", async () => {
      const user = userEvent.setup();
      const view = popoverView("x\n- [ ] t", { mode, room });
      rightClick(box());
      await menu();
      await user.click(
        screen.getByRole("menuitemradio", { name: "Not applicable" }),
      );
      expect(bytes(view.state)).toBe("x\n- [~] t");
      expect(screen.queryByRole("menu")).toBeNull();
      expect(document.activeElement).toBe(view.contentDOM);
      undoOnce(view);
      expect(bytes(view.state)).toBe("x\n- [ ] t");
    });

    it("opens on Shift+F10 on the task line and picks with the arrow keys", async () => {
      const user = userEvent.setup();
      const view = popoverView("x\n- [x] t", { mode, room });
      setCaret(view, 8);
      keys(view, "Shift-F10");
      await menu();
      expect(screen.getByRole("menuitemradio", { name: "Done" })).toHaveFocus();
      await user.keyboard("{ArrowDown}");
      expect(
        screen.getByRole("menuitemradio", { name: "Not applicable" }),
      ).toHaveFocus();
      await user.keyboard("{ArrowDown}");
      expect(screen.getByRole("menuitemradio", { name: "Open" })).toHaveFocus();
      await user.keyboard("{ArrowUp}{End}");
      expect(
        screen.getByRole("menuitemradio", { name: "Not applicable" }),
      ).toHaveFocus();
      await user.keyboard("{Home}{Enter}");
      expect(bytes(view.state)).toBe("x\n- [ ] t");
      undoOnce(view);
      expect(bytes(view.state)).toBe("x\n- [x] t");
    });

    it("writes nothing on Escape", async () => {
      const user = userEvent.setup();
      const view = popoverView("x\n- [ ] t", { mode, room });
      setCaret(view, 8);
      keys(view, "Shift-F10");
      await menu();
      await user.keyboard("{ArrowDown}{Escape}");
      expect(screen.queryByRole("menu")).toBeNull();
      expect(bytes(view.state)).toBe("x\n- [ ] t");
      expect(document.activeElement).toBe(view.contentDOM);
    });
  },
);

describe("the task menu and its peers", () => {
  it("writes nothing to the task next to it when a peer deleted it", async () => {
    const { local, peer, sync } = popoverPeers("x\n- [ ] a\n- [ ] b");
    setCaret(local, 8);
    keys(local, "Shift-F10");
    await menu();
    peer.dispatch({ changes: { from: 2, to: 10 }, userEvent: "delete" });
    sync();
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
    expect(bytes(local.state)).toBe("x\n- [ ] b");
  });
});

describe("a page that takes no writes (C12)", () => {
  it.each(["right click", "Shift+F10"])(
    "opens nothing from a %s",
    async (how) => {
      const view = popoverView("x\n- [ ] t", { adapter: "reflection" });
      view.dispatch({
        effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
      });
      if (how === "right click") {
        rightClick(box());
      } else {
        setCaret(view, 8);
        keys(view, "Shift-F10");
      }
      await Promise.resolve();
      expect(screen.queryByRole("menu")).toBeNull();
      expect(bytes(view.state)).toBe("x\n- [ ] t");
    },
  );

  it("closes without writing when the page stops taking writes", async () => {
    const view = popoverView("x\n- [ ] t", { adapter: "reflection" });
    rightClick(box());
    await menu();
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
    expect(bytes(view.state)).toBe("x\n- [ ] t");
  });
});

describe("Ctrl+click on a Mac", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("opens the menu and does not flip the box", async () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    const view = popoverView("x\n- [ ] t");
    const el = box();
    // The order macOS sends a Ctrl+click in.
    for (const type of ["mousedown", "contextmenu", "mouseup", "click"]) {
      el.dispatchEvent(
        new MouseEvent(type, {
          bubbles: true,
          cancelable: true,
          button: type === "contextmenu" ? 2 : 0,
          ctrlKey: true,
        }),
      );
    }
    await menu();
    expect(bytes(view.state)).toBe("x\n- [ ] t");
  });
});
