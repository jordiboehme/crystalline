/**
 * The redo key suite (`redoKeySuite`) on a Mac: CodeMirror and w3c-keyname read
 * `navigator.platform` once, when they load, so it is set before any import.
 */

import { describe, expect, test, vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "MacIntel",
    configurable: true,
  });
});

import { redoKeySuite } from "../test/redoKeySuite";
import { isShiftedZ } from "./undoStack";

redoKeySuite();

describe("isShiftedZ on a Mac (Mod is Cmd)", () => {
  const down = (init: KeyboardEventInit): KeyboardEvent =>
    new KeyboardEvent("keydown", { bubbles: true, ...init });

  test("Cmd+Shift+Z in either case, and not Ctrl+Shift+Z", () => {
    for (const key of ["z", "Z"]) {
      expect(
        isShiftedZ(down({ key, keyCode: 90, metaKey: true, shiftKey: true })),
      ).toBe(true);
    }
    expect(
      isShiftedZ(
        down({ key: "z", keyCode: 90, ctrlKey: true, shiftKey: true }),
      ),
    ).toBe(false);
  });
});
