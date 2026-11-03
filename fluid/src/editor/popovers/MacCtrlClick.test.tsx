/**
 * A Ctrl+click on a Mac is the browser's context menu. No drawn element
 * acts on it: the chevron writes no fold sign, and a pill, a formula, the
 * kind icon, a link or a chip asks for no popover (and a link opens no tab).
 * A plain press on each still asks, so the test sees the guard and not a
 * missing element.
 */

import type { EditorView } from "@codemirror/view";
import { cleanup, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { bytes, cleanupMarkViews, markView } from "../../test/markKit";
import type { ElementEdit } from "../elementEdit";

beforeEach(() => {
  vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
});

afterEach(() => {
  cleanup();
  cleanupMarkViews();
  vi.restoreAllMocks();
});

/** The events macOS sends for a Ctrl+click, in their order. */
function ctrlClick(el: Element): void {
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
}

function press(el: Element): void {
  el.dispatchEvent(
    new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }),
  );
}

function listened(text: string): { view: EditorView; heard: ElementEdit[] } {
  const heard: ElementEdit[] = [];
  const view = markView(text, {
    onElementEdit: (edit) => {
      if (edit) heard.push(edit);
    },
  });
  return { view, heard };
}

async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

describe("a Ctrl+click on a Mac", () => {
  it("writes no fold sign from the chevron", async () => {
    const text = "x\n> [!NOTE]+ T\n> b";
    const { view } = listened(text);
    ctrlClick(await screen.findByRole("button", { name: "Callout fold" }));
    await settle();
    expect(bytes(view.state)).toBe(text);
  });

  it.each([
    [
      "the kind icon",
      "x\n> [!NOTE]+ T\n> b",
      'button[aria-label="Callout kind: Note"]',
    ],
    ["a pill", "x\n:status[Done]", ".status-label"],
    ["a formula", "x\na $`y`$", ".cm-math-inline"],
    ["a link", "x\n[t](https://a.b)", ".cm-link-text"],
    ["a chip", "x\n[[Old]]", ".cm-wikilink"],
  ])("asks for no popover from %s", async (_name, text, selector) => {
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    const { view, heard } = listened(text);
    let el: Element | null = null;
    await waitFor(() => {
      el = view.contentDOM.querySelector(selector);
      expect(el).not.toBeNull();
    });
    ctrlClick(el!);
    await settle();
    expect(heard).toEqual([]);
    expect(open).not.toHaveBeenCalled();
    expect(bytes(view.state)).toBe(text);
    // A plain press on the same element does ask.
    press(view.contentDOM.querySelector(selector)!);
    await settle();
    expect(heard).toHaveLength(1);
  });
});
