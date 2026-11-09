/**
 * Which keydowns count as the shifted-Z redo chord (`isShiftedZ`). The redo
 * itself, per editor and platform, is `redoKeySuite` in the
 * `undoStack.<platform>.test.ts` files.
 */

import { describe, expect, test } from "vitest";

import { isShiftedZ } from "./undoStack";

function down(init: KeyboardEventInit): KeyboardEvent {
  return new KeyboardEvent("keydown", { bubbles: true, ...init });
}

describe("isShiftedZ on Linux and Windows (Mod is Ctrl)", () => {
  test("either case of z with Ctrl and Shift", () => {
    for (const key of ["z", "Z"]) {
      expect(
        isShiftedZ(down({ key, keyCode: 90, ctrlKey: true, shiftKey: true })),
      ).toBe(true);
    }
  });

  test("not without Shift, without Ctrl, with Alt (AltGr) or with Meta", () => {
    expect(isShiftedZ(down({ key: "z", keyCode: 90, ctrlKey: true }))).toBe(
      false,
    );
    expect(isShiftedZ(down({ key: "Z", keyCode: 90, shiftKey: true }))).toBe(
      false,
    );
    expect(
      isShiftedZ(
        down({
          key: "z",
          keyCode: 90,
          ctrlKey: true,
          shiftKey: true,
          altKey: true,
        }),
      ),
    ).toBe(false);
    expect(
      isShiftedZ(
        down({
          key: "z",
          keyCode: 90,
          ctrlKey: true,
          metaKey: true,
          shiftKey: true,
        }),
      ),
    ).toBe(false);
  });

  test("never while an input method composes", () => {
    expect(
      isShiftedZ(
        down({
          key: "z",
          keyCode: 90,
          ctrlKey: true,
          shiftKey: true,
          isComposing: true,
        }),
      ),
    ).toBe(false);
    expect(
      isShiftedZ(
        down({ key: "Process", keyCode: 229, ctrlKey: true, shiftKey: true }),
      ),
    ).toBe(false);
  });

  test("the letter decides on a Latin layout, the key position elsewhere", () => {
    // QWERTZ: the key where QWERTY has Z types y, and stays Ctrl+Shift+Y.
    expect(
      isShiftedZ(
        down({
          key: "Y",
          code: "KeyZ",
          keyCode: 89,
          ctrlKey: true,
          shiftKey: true,
        }),
      ),
    ).toBe(false);
    // A Cyrillic layout types a letter CodeMirror maps back by keyCode.
    expect(
      isShiftedZ(
        down({
          key: "Я",
          code: "KeyZ",
          keyCode: 90,
          ctrlKey: true,
          shiftKey: true,
        }),
      ),
    ).toBe(true);
  });
});
