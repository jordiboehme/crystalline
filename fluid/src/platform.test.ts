/**
 * `platform.ts` off a Mac, read once when it loads, so the platform is set
 * before any import. `platform.mac.test.ts` is the Mac's side.
 */

import { describe, expect, it, vi } from "vitest";

// A Windows platform with a user agent that names a Mac: the platform decides,
// as it does for CodeMirror's `Mod`, so a label never disagrees with the key.
vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "Win32",
    configurable: true,
  });
  Object.defineProperty(navigator, "userAgent", {
    value:
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
    configurable: true,
  });
});

import { isMac, isMenuKey, keyLabel } from "./platform";

const key = (key: string, init: KeyboardEventInit = {}) =>
  new KeyboardEvent("keydown", { key, ...init });

describe("off a Mac", () => {
  it("is not a Mac", () => {
    expect(isMac()).toBe(false);
  });

  it("takes Shift+F10 and the menu key as the menu key, and Ctrl+Enter not", () => {
    expect(isMenuKey(key("F10", { shiftKey: true }))).toBe(true);
    expect(isMenuKey(key("ContextMenu"))).toBe(true);
    expect(isMenuKey(key("ContextMenu", { shiftKey: true }))).toBe(true);
    expect(isMenuKey(key("Enter", { ctrlKey: true }))).toBe(false);
    expect(isMenuKey(key("F10"))).toBe(false);
    expect(isMenuKey(key("F10", { shiftKey: true, altKey: true }))).toBe(false);
    expect(isMenuKey(key("F10", { shiftKey: true, ctrlKey: true }))).toBe(
      false,
    );
    expect(isMenuKey(key("ContextMenu", { metaKey: true }))).toBe(false);
  });

  it("names keys with Ctrl, Alt and Shift in words", () => {
    expect(keyLabel("Mod-Shift-z")).toBe("Ctrl+Shift+Z");
    expect(keyLabel("Mod-k")).toBe("Ctrl+K");
    expect(keyLabel("Alt-F10")).toBe("Alt+F10");
    expect(keyLabel("Alt-F10", { words: true })).toBe("Alt+F10");
    expect(keyLabel("Shift-F10")).toBe("Shift+F10");
    expect(keyLabel("Alt-ArrowUp")).toBe("Alt+Up Arrow");
    expect(keyLabel("Ctrl-Enter")).toBe("Ctrl+Enter");
    expect(keyLabel("Mod--")).toBe("Ctrl+-");
    expect(keyLabel("?")).toBe("?");
  });
});
