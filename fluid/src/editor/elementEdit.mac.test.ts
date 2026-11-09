/**
 * The element popover's menu key and Ctrl+click on a Mac. CodeMirror and
 * `platform.ts` read `navigator.platform` once, when they load, so it is set
 * before any import.
 *
 * A Mac has no menu key and F10 needs fn, so Ctrl+Return opens the popover of
 * the element at the caret too (R111). With no element there it goes on.
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
} from "../test/markKit";
import type { ElementEdit } from "./elementEdit";

afterEach(cleanupMarkViews);

const CODE = "a\n\n```ts\nx\n```\n\nb";

function listened(text: string): { view: EditorView; heard: ElementEdit[] } {
  const heard: ElementEdit[] = [];
  const view = markView(text, {
    onElementEdit: (edit) => {
      if (edit) heard.push(edit);
    },
  });
  return { view, heard };
}

describe("Ctrl+Return on a Mac", () => {
  it("asks for the link's popover in a link", () => {
    const { view, heard } = listened("a [t](u) b");
    setCaret(view, 3);
    expect(keys(view, "Ctrl-Enter")).toBe(true);
    expect(heard.at(-1)).toMatchObject({ kind: "link" });
    expect(bytes(view.state)).toBe("a [t](u) b");
  });

  it("asks for the code block's actions on a code line", () => {
    const { view, heard } = listened(CODE);
    setCaret(view, 9);
    expect(keys(view, "Ctrl-Enter")).toBe(true);
    expect(heard.at(-1)).toMatchObject({ kind: "code-actions", at: 3 });
    expect(bytes(view.state)).toBe(CODE);
  });

  it("goes on where no element is, and writes nothing", () => {
    const { view, heard } = listened("plain words");
    setCaret(view, 3);
    expect(keys(view, "Ctrl-Enter")).toBe(false);
    expect(heard).toEqual([]);
    expect(bytes(view.state)).toBe("plain words");
  });

  it("Shift+F10 still asks", () => {
    const { view, heard } = listened("a [t](u) b");
    setCaret(view, 3);
    expect(keys(view, "Shift-F10")).toBe(true);
    expect(heard.at(-1)).toMatchObject({ kind: "link" });
  });
});

describe("a Ctrl+click on a code block's header buttons on a Mac", () => {
  function press(el: Element): void {
    el.dispatchEvent(
      new MouseEvent("mousedown", {
        bubbles: true,
        cancelable: true,
        button: 0,
      }),
    );
  }

  function button(view: EditorView, selector: string): HTMLElement {
    const el = view.contentDOM.querySelector<HTMLElement>(selector);
    if (!el) throw new Error(`no ${selector}`);
    return el;
  }

  it("asks for nothing, and a plain press still asks", () => {
    const { view, heard } = listened(CODE);
    const language = button(view, 'button[aria-label^="Code language"]');
    const more = button(view, 'button[aria-label="More code block actions"]');
    for (const el of [language, more]) {
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
    expect(heard).toEqual([]);
    press(language);
    expect(heard).toHaveLength(1);
  });
});
