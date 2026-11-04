/**
 * The popover host: where it opens and what it focuses, what commits (Enter,
 * Save), what writes nothing (Escape, a press outside, the focus leaving),
 * what closes it on its own (the setting, Raw, a peer deleting the element,
 * a page that stops taking writes) and where the focus goes back to.
 */

import { EditorState, StateEffect } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  setCaret,
  setRaw,
  setShowMarks,
  undoOnce,
} from "../../test/markKit";
import { popoverPeers, popoverView } from "../../test/popoverKit";
import { placement } from "./placement";

afterEach(() => {
  cleanup();
  cleanupMarkViews();
});

type Mode = "hidden" | "show";
const MODES: Mode[] = ["hidden", "show"];

function select(view: EditorView, from: number, to: number): void {
  view.dispatch({ selection: { anchor: from, head: to }, userEvent: "select" });
}

async function linkDialog(): Promise<HTMLElement> {
  return screen.findByRole("dialog", { name: "Link" });
}

describe.each([false, true])("the Link popover (room: %s)", (room) => {
  it.each(MODES)(
    "opens on Mod-k with the focus in Address and commits on Enter as one undo step (%s)",
    async (mode) => {
      const user = userEvent.setup();
      const view = popoverView("go there", { mode, room });
      select(view, 3, 8);
      keys(view, "Mod-k");
      await linkDialog();
      const field = screen.getByRole("textbox", { name: "Address" });
      expect(field).toHaveFocus();
      await user.type(field, "https://x{Enter}");
      expect(bytes(view.state)).toBe("go [there](https://x)");
      expect(screen.queryByRole("dialog")).toBeNull();
      // The focus is back in the editor, with the caret after the link.
      expect(view.hasFocus || document.activeElement === view.contentDOM).toBe(
        true,
      );
      expect(caret(view)).toBeGreaterThanOrEqual(8);
      undoOnce(view);
      expect(bytes(view.state)).toBe("go there");
    },
  );

  it.each(MODES)("commits on Save (%s)", async (mode) => {
    const user = userEvent.setup();
    const view = popoverView("go there", { mode, room });
    select(view, 3, 8);
    keys(view, "Mod-k");
    await linkDialog();
    await user.type(
      screen.getByRole("textbox", { name: "Address" }),
      "https://x",
    );
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(bytes(view.state)).toBe("go [there](https://x)");
  });

  it.each(MODES)(
    "writes nothing on Escape and hands the focus back (%s)",
    async (mode) => {
      const user = userEvent.setup();
      const view = popoverView("go there", { mode, room });
      select(view, 3, 8);
      keys(view, "Mod-k");
      await linkDialog();
      await user.type(
        screen.getByRole("textbox", { name: "Address" }),
        "https://x",
      );
      await user.keyboard("{Escape}");
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(bytes(view.state)).toBe("go there");
      expect(document.activeElement).toBe(view.contentDOM);
      expect(view.state.selection.main).toMatchObject({ from: 3, to: 8 });
    },
  );

  it.each(MODES)("writes nothing on a press outside (%s)", async (mode) => {
    const user = userEvent.setup();
    const view = popoverView("go there", { mode, room });
    select(view, 3, 8);
    keys(view, "Mod-k");
    await linkDialog();
    await user.type(
      screen.getByRole("textbox", { name: "Address" }),
      "https://x",
    );
    await user.pointer({ keys: "[MouseLeft]", target: document.body });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(bytes(view.state)).toBe("go there");
  });

  it.each(MODES)(
    "writes nothing when the focus leaves it (%s)",
    async (mode) => {
      const view = popoverView("go there", { mode, room });
      select(view, 3, 8);
      keys(view, "Mod-k");
      await linkDialog();
      const outside = document.createElement("button");
      document.body.appendChild(outside);
      outside.focus();
      await waitFor(() => {
        expect(screen.queryByRole("dialog")).toBeNull();
      });
      expect(bytes(view.state)).toBe("go there");
      outside.remove();
    },
  );

  it.each(MODES)(
    "closes without writing when the setting flips (%s)",
    async (mode) => {
      const user = userEvent.setup();
      const view = popoverView("go there", { mode, room });
      select(view, 3, 8);
      keys(view, "Mod-k");
      await linkDialog();
      await user.type(
        screen.getByRole("textbox", { name: "Address" }),
        "https://x",
      );
      setShowMarks(view, mode === "hidden");
      await waitFor(() => {
        expect(screen.queryByRole("dialog")).toBeNull();
      });
      expect(bytes(view.state)).toBe("go there");
      // The focus goes back to the view that asked, not to the page.
      expect(document.activeElement).toBe(view.contentDOM);
    },
  );

  it("closes without writing when Raw flips", async () => {
    const view = popoverView("go there", { room });
    select(view, 3, 8);
    keys(view, "Mod-k");
    await linkDialog();
    setRaw(view, true);
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(bytes(view.state)).toBe("go there");
  });

  it("closes without writing when the page stops taking writes", async () => {
    const view = popoverView("[t](u)", { room });
    setCaret(view, 1);
    keys(view, "Mod-k");
    await linkDialog();
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(bytes(view.state)).toBe("[t](u)");
  });
});

describe("the host and the keyboard", () => {
  it.each(MODES)(
    "opens on Shift+F10 in a link; Escape returns the focus with the caret where it was (%s)",
    async (mode) => {
      const user = userEvent.setup();
      const view = popoverView("a [t](u) b", { mode });
      view.focus();
      setCaret(view, 3);
      const before = caret(view);
      keys(view, "Shift-F10");
      await linkDialog();
      expect(screen.getByRole("textbox", { name: "Address" })).toHaveFocus();
      await user.keyboard("{Escape}");
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(document.activeElement).toBe(view.contentDOM);
      expect(caret(view)).toBe(before);
      expect(bytes(view.state)).toBe("a [t](u) b");
    },
  );

  it("closes without writing when a peer deleted the link", async () => {
    const user = userEvent.setup();
    const { local, peer, sync } = popoverPeers("a [t](u) b");
    setCaret(local, 3);
    keys(local, "Mod-k");
    await linkDialog();
    await user.type(screen.getByRole("textbox", { name: "Address" }), "v");
    peer.dispatch({ changes: { from: 2, to: 8 }, userEvent: "delete" });
    sync();
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(bytes(local.state)).toBe("a  b");
    expect(document.activeElement).toBe(local.contentDOM);
  });

  it("leaves the focus where it went when it closes after the person left it", async () => {
    const view = popoverView("go there");
    select(view, 3, 8);
    keys(view, "Mod-k");
    await linkDialog();
    const outside = document.createElement("button");
    document.body.appendChild(outside);
    outside.focus();
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(document.activeElement).toBe(outside);
    outside.remove();
  });
});

describe("placement", () => {
  const box = { top: 100, left: 50, width: 600 };
  const size = { width: 300, height: 120 };

  it("goes under the element, inside its column", () => {
    expect(
      placement({ top: 200, bottom: 220, left: 120 }, size, box, 900),
    ).toEqual({ top: 126, left: 70 });
  });

  it("goes above the element with no room below", () => {
    expect(
      placement({ top: 800, bottom: 820, left: 120 }, size, box, 900),
    ).toEqual({ top: 574, left: 70 });
  });

  it("stays under the sticky bar when there is no room above either", () => {
    expect(
      placement({ top: 150, bottom: 820, left: 120 }, size, box, 900, 140),
    ).toEqual({ top: 726, left: 70 });
  });

  it("keeps its right edge inside the column", () => {
    expect(
      placement({ top: 200, bottom: 220, left: 600 }, size, box, 900).left,
    ).toBe(300);
  });
});
