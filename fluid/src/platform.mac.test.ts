/**
 * `platform.ts` on a Mac. It reads `navigator.platform` once, when it loads,
 * as CodeMirror does, so the platform is set before any import.
 */

import { describe, expect, it, vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "MacIntel",
    configurable: true,
  });
});

import { isMac, isMenuKey, keyLabel } from "./platform";

const key = (key: string, init: KeyboardEventInit = {}) =>
  new KeyboardEvent("keydown", { key, ...init });

describe("on a Mac", () => {
  it("is a Mac", () => {
    expect(isMac()).toBe(true);
  });

  it("takes Ctrl+Return as the menu key too, and no other Return", () => {
    expect(isMenuKey(key("Enter", { ctrlKey: true }))).toBe(true);
    expect(isMenuKey(key("F10", { shiftKey: true }))).toBe(true);
    expect(isMenuKey(key("ContextMenu"))).toBe(true);
    expect(isMenuKey(key("Enter"))).toBe(false);
    expect(isMenuKey(key("Enter", { metaKey: true }))).toBe(false);
    expect(isMenuKey(key("Enter", { ctrlKey: true, metaKey: true }))).toBe(
      false,
    );
    expect(isMenuKey(key("Enter", { ctrlKey: true, altKey: true }))).toBe(
      false,
    );
    expect(isMenuKey(key("Enter", { ctrlKey: true, shiftKey: true }))).toBe(
      false,
    );
  });

  it("names keys with the Mac's signs, in Apple's order", () => {
    expect(keyLabel("Mod-Shift-z")).toBe("⇧⌘Z");
    expect(keyLabel("Mod-k")).toBe("⌘K");
    expect(keyLabel("Alt-F10")).toBe("⌥F10");
    expect(keyLabel("Ctrl-Enter")).toBe("⌃↩");
    expect(keyLabel("Alt-ArrowUp")).toBe("⌥↑");
    expect(keyLabel("Ctrl-Alt-Shift-Mod-x")).toBe("⌃⌥⇧⌘X");
  });

  it("names keys in words for a sentence or a screen reader", () => {
    expect(keyLabel("Alt-F10", { words: true })).toBe("Option+F10");
    expect(keyLabel("Ctrl-Enter", { words: true })).toBe("Ctrl+Return");
    expect(keyLabel("Alt-ArrowUp", { words: true })).toBe("Option+Up Arrow");
    expect(keyLabel("Mod-k", { words: true })).toBe("Cmd+K");
    expect(keyLabel("Mod-Backspace", { words: true })).toBe("Cmd+Delete");
  });
});
