import { describe, expect, it } from "vitest";

import { flipRows } from "./text";

describe("flipRows", () => {
  it("turns the image upside down so row 0 is the bottom, as GL expects", () => {
    // 1 x 2 image: top pixel red, bottom pixel blue.
    const data = new Uint8ClampedArray([255, 0, 0, 255, 0, 0, 255, 255]);
    expect(Array.from(flipRows(data, 1, 2))).toEqual([
      0, 0, 255, 255, 255, 0, 0, 255,
    ]);
  });
});
