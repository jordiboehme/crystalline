/**
 * The word and line deletes over hidden marks on a Mac: Option+Backspace and
 * Option+Delete take a word, Cmd+Backspace and Cmd+Delete take the rest of the
 * line. CodeMirror reads `navigator.platform` once, when it loads, so it is set
 * before any import.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "MacIntel",
    configurable: true,
  });
});

import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
} from "../../test/markKit";

afterEach(cleanupMarkViews);

describe.each([
  { name: "solo", room: false },
  { name: "in a room", room: true },
])("on a Mac, $name", ({ room }) => {
  it("Cmd+Backspace after a link's hidden address takes the line to its start", () => {
    const v = markView("x [a](u)\nnext", { room });
    setCaret(v, 8);
    expect(caret(v)).toBe(8);
    expect(keys(v, "Mod-Backspace")).toBe(true);
    expect(bytes(v.state)).toBe("\nnext");
  });

  it("Cmd+Delete before a link takes the line to its end", () => {
    const v = markView("[a](u) x\nnext", { room });
    setCaret(v, 0);
    expect(keys(v, "Mod-Delete")).toBe(true);
    expect(bytes(v.state)).toBe("\nnext");
  });

  it("Option+Backspace after a link's hidden address takes the link text", () => {
    const v = markView("x [a](u)", { room });
    setCaret(v, 8);
    keys(v, "Alt-Backspace");
    expect(bytes(v.state)).toBe("x ");
  });
});
