/**
 * A callout's head: the kind icon is a button that opens the "Callout kind"
 * menu, and the chevron is a button that flips the fold sign. Each pick and
 * each click writes only the kind word or the sign, as one undo step.
 */

import { EditorState, StateEffect } from "@codemirror/state";
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

const TEXT = "x\n> [!NOTE]+ T\n> b";

function press(el: Element): MouseEvent {
  const event = new MouseEvent("mousedown", {
    bubbles: true,
    cancelable: true,
    button: 0,
  });
  el.dispatchEvent(event);
  return event;
}

async function menu(): Promise<HTMLElement> {
  return screen.findByRole("menu", { name: "Callout kind" });
}

describe.each(CASES)(
  "a callout head ($mode mode, room $room)",
  ({ mode, room }) => {
    it("flips the fold sign from the chevron as one undo step", async () => {
      const view = popoverView(TEXT, { mode, room });
      const chevron = await screen.findByRole("button", {
        name: "Callout fold",
      });
      expect(chevron).toHaveAttribute("aria-expanded", "true");
      const event = press(chevron);
      expect(event.defaultPrevented).toBe(true);
      expect(bytes(view.state)).toBe("x\n> [!NOTE]- T\n> b");
      await waitFor(() => {
        expect(
          screen.getByRole("button", { name: "Callout fold" }),
        ).toHaveAttribute("aria-expanded", "false");
      });
      undoOnce(view);
      expect(bytes(view.state)).toBe(TEXT);
    });

    it("opens the kind menu from the icon and writes the kind", async () => {
      const user = userEvent.setup();
      const view = popoverView(TEXT, { mode, room });
      press(await screen.findByRole("button", { name: "Callout kind: Note" }));
      await menu();
      expect(
        screen.getAllByRole("menuitemradio").map((item) => item.textContent),
      ).toEqual([
        "Note",
        "Tip",
        "Important",
        "Warning",
        "Caution",
        "Details",
        "Fold: none",
        "Fold: open",
        "Fold: closed",
      ]);
      const note = screen.getByRole("menuitemradio", { name: "Note" });
      expect(note).toHaveAttribute("aria-checked", "true");
      expect(note).toHaveFocus();
      expect(
        screen.getByRole("menuitemradio", { name: "Fold: open" }),
      ).toHaveAttribute("aria-checked", "true");
      await user.click(screen.getByRole("menuitemradio", { name: "Warning" }));
      expect(bytes(view.state)).toBe("x\n> [!WARNING]+ T\n> b");
      expect(document.activeElement).toBe(view.contentDOM);
      undoOnce(view);
      expect(bytes(view.state)).toBe(TEXT);
    });

    it("takes the fold sign away with Fold: none", async () => {
      const user = userEvent.setup();
      const view = popoverView(TEXT, { mode, room });
      press(await screen.findByRole("button", { name: "Callout kind: Note" }));
      await menu();
      await user.click(
        screen.getByRole("menuitemradio", { name: "Fold: none" }),
      );
      expect(bytes(view.state)).toBe("x\n> [!NOTE] T\n> b");
      undoOnce(view);
      expect(bytes(view.state)).toBe(TEXT);
    });

    it("offers no Fold: none for a details callout", async () => {
      popoverView("x\n> [!DETAILS] T\n> b", { mode, room });
      await screen.findByRole("button", { name: "Callout fold" });
      // The chevron is a details callout's icon, so the menu opens from the
      // head line with Shift+F10.
      expect(
        screen.queryByRole("button", { name: /^Callout kind/ }),
      ).toBeNull();
    });
  },
);

describe("the kind menu from the keyboard", () => {
  it("opens on Shift+F10 on the head line and moves with the arrow keys", async () => {
    const user = userEvent.setup();
    const view = popoverView("x\n> [!DETAILS] T\n> b");
    setCaret(view, 15);
    keys(view, "Shift-F10");
    await menu();
    expect(
      screen.queryByRole("menuitemradio", { name: "Fold: none" }),
    ).toBeNull();
    expect(
      screen.getByRole("menuitemradio", { name: "Fold: closed" }),
    ).toHaveAttribute("aria-checked", "true");
    expect(
      screen.getByRole("menuitemradio", { name: "Details" }),
    ).toHaveFocus();
    await user.keyboard("{ArrowUp}{Enter}");
    expect(bytes(view.state)).toBe("x\n> [!CAUTION]- T\n> b");
  });

  it("writes nothing for the kind already checked", async () => {
    const user = userEvent.setup();
    const view = popoverView(TEXT);
    setCaret(view, 14);
    keys(view, "Shift-F10");
    await menu();
    await user.keyboard("{Enter}");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(bytes(view.state)).toBe(TEXT);
  });
});

describe("the kind menu and its peers", () => {
  it("writes nothing to the callout next to it when a peer deleted it", async () => {
    const { local, peer, sync } = popoverPeers(
      "x\n\n> [!NOTE] a\n\n> [!TIP] b",
    );
    setCaret(local, 12);
    keys(local, "Shift-F10");
    await menu();
    peer.dispatch({ changes: { from: 3, to: 16 }, userEvent: "delete" });
    sync();
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
    expect(bytes(local.state)).toBe("x\n\n> [!TIP] b");
  });
});

describe("a page that takes no writes (C12)", () => {
  it("writes nothing from the chevron and opens nothing from the icon", async () => {
    const view = popoverView(TEXT, { adapter: "reflection" });
    const chevron = await screen.findByRole("button", { name: "Callout fold" });
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    press(chevron);
    press(screen.getByRole("button", { name: "Callout kind: Note" }));
    await Promise.resolve();
    expect(bytes(view.state)).toBe(TEXT);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("closes the menu without writing when the page stops taking writes", async () => {
    const view = popoverView(TEXT, { adapter: "reflection" });
    press(await screen.findByRole("button", { name: "Callout kind: Note" }));
    await menu();
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
    expect(bytes(view.state)).toBe(TEXT);
  });
});
