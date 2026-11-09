/**
 * The "Task state" menu on a Mac: a Ctrl+click on the box and Ctrl+Return on
 * the task line open it, and neither flips the box. The platform is read once,
 * when the modules load, so it is set before any import.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "MacIntel",
    configurable: true,
  });
});

import { cleanup, screen } from "@testing-library/react";

import { bytes, cleanupMarkViews, keys, setCaret } from "../../test/markKit";
import { popoverView } from "../../test/popoverKit";

afterEach(() => {
  cleanup();
  cleanupMarkViews();
});

function box(): Element {
  const el = document.querySelector(
    ".cm-content .cm-task-toggle, .cm-content .cm-task-na",
  );
  if (!el) throw new Error("no task box is drawn");
  return el;
}

async function menu(): Promise<HTMLElement> {
  return screen.findByRole("menu", { name: "Task state" });
}

describe("the task menu on a Mac", () => {
  it("opens on a Ctrl+click and does not flip the box", async () => {
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

  it("opens on Ctrl+Return on the task line, with the state checked", async () => {
    const view = popoverView("x\n- [x] t");
    setCaret(view, 8);
    expect(keys(view, "Ctrl-Enter")).toBe(true);
    await menu();
    expect(screen.getByRole("menuitemradio", { name: "Done" })).toHaveFocus();
    expect(bytes(view.state)).toBe("x\n- [x] t");
  });
});
