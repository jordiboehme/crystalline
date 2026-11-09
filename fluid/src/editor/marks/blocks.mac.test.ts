/**
 * A task's keys and its Ctrl+click on a Mac. CodeMirror and `platform.ts`
 * read `navigator.platform` once, when they load, so it is set before any
 * import.
 *
 * Cmd+Return flips a task there. Ctrl+Return is the Mac's menu key (R111): it
 * opens the task menu and flips nothing. A Ctrl+click (`macMenuClick`) is the
 * browser's context menu, so the box opens the task menu and writes nothing.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "MacIntel",
    configurable: true,
  });
});

import type { EditorView } from "@codemirror/view";

import {
  bytes,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
} from "../../test/markKit";
import type { ElementEdit } from "../elementEdit";
import { macMenuClick } from "../elementEdit";

afterEach(cleanupMarkViews);

/** The box of the task line. */
function box(view: EditorView): HTMLElement {
  const found = view.contentDOM.querySelector<HTMLElement>(
    "input.cm-task-toggle, .cm-task-na",
  );
  if (!found) throw new Error("no task box is drawn");
  return found;
}

function listened(doc: string) {
  const heard: ElementEdit[] = [];
  const view = markView(doc, {
    onElementEdit: (edit) => {
      if (edit) heard.push(edit);
    },
  });
  return { view, heard };
}

describe("a task's keys on a Mac", () => {
  it("Cmd+Return flips a task", () => {
    const { view } = listened("- [ ] t");
    setCaret(view, 7);
    expect(keys(view, "Mod-Enter")).toBe(true);
    expect(bytes(view.state)).toBe("- [x] t");
  });

  it("Ctrl+Return flips no task and opens its menu", () => {
    const { view, heard } = listened("- [ ] t");
    setCaret(view, 7);
    expect(keys(view, "Ctrl-Enter")).toBe(true);
    expect(bytes(view.state)).toBe("- [ ] t");
    expect(heard.at(-1)).toMatchObject({ kind: "task", at: 2 });
  });
});

describe("a Ctrl+click on a Mac", () => {
  function ctrlClick(el: HTMLElement): void {
    for (const type of ["mousedown", "click", "contextmenu"]) {
      el.dispatchEvent(
        new MouseEvent(type, {
          bubbles: true,
          cancelable: true,
          button: 0,
          ctrlKey: true,
        }),
      );
    }
  }

  it("reads a Ctrl+click as the menu's, and a plain press not", () => {
    expect(macMenuClick(new MouseEvent("mousedown", { ctrlKey: true }))).toBe(
      true,
    );
    expect(macMenuClick(new MouseEvent("mousedown"))).toBe(false);
  });

  it("opens the task menu and does not flip the box", () => {
    const { view, heard } = listened("- [ ] t");
    ctrlClick(box(view));
    expect(bytes(view.state)).toBe("- [ ] t");
    expect(heard.at(-1)).toMatchObject({ kind: "task", at: 2 });
  });
});
