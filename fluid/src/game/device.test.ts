import { describe, expect, it } from "vitest";

import { backbufferSize, refusalReason } from "./device";

describe("refusalReason", () => {
  it("refuses without WebGL2 and on touch-only devices", () => {
    expect(refusalReason({ webgl2: false, coarseOnly: false })).toBe(
      "no-webgl2",
    );
    expect(refusalReason({ webgl2: true, coarseOnly: true })).toBe("touch");
    expect(refusalReason({ webgl2: true, coarseOnly: false })).toBeNull();
  });
});

describe("backbufferSize", () => {
  it("follows the CSS size times the device pixel ratio, capped at 1.5", () => {
    expect(backbufferSize(1000, 500, 1, 1)).toEqual({
      width: 1000,
      height: 500,
    });
    expect(backbufferSize(1000, 500, 2, 1)).toEqual({
      width: 1500,
      height: 750,
    });
  });

  it("applies the render scale and never returns zero", () => {
    expect(backbufferSize(1000, 500, 1, 0.5)).toEqual({
      width: 500,
      height: 250,
    });
    expect(backbufferSize(0, 0, 2, 1)).toEqual({ width: 1, height: 1 });
  });
});
