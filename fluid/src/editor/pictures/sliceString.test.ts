/**
 * The picture code reads the text with `doc.sliceString`, never with
 * `state.sliceDoc` (Global Constraints, M2): `sliceDoc` joins lines with the
 * state's line break, so a reference read across a CRLF line would not be
 * the bytes it stands for. Every reference drawn today is on one line, so
 * the two agree; this guard keeps it so when a reference ever spans lines.
 */

import { describe, expect, it } from "vitest";

const SOURCES = import.meta.glob<string>(
  [
    "./*.ts",
    "!./*.test.ts",
    "../imagePreviews.ts",
    "../PictureControls.tsx",
    "../focusBack.ts",
  ],
  { query: "?raw", import: "default", eager: true },
);

describe("the picture code", () => {
  it("is read", () => {
    expect(Object.keys(SOURCES)).toEqual(
      expect.arrayContaining([
        "./verbs.ts",
        "./dragMove.ts",
        "./selectKeys.ts",
        "../imagePreviews.ts",
        "../PictureControls.tsx",
      ]),
    );
  });

  it.each(Object.entries(SOURCES))(
    "%s reads text with sliceString",
    (_, src) => {
      expect(src.match(/\.sliceDoc\(/g) ?? []).toEqual([]);
    },
  );
});
