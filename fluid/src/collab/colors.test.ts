import { describe, expect, it } from "vitest";

import { presenceColor } from "./colors";

describe("presenceColor", () => {
  it("is deterministic and well-formed", () => {
    const first = presenceColor("ada");
    expect(presenceColor("ada")).toEqual(first);
    expect(first.color).toMatch(/^#[0-9a-f]{6}$/);
    expect(first.colorLight).toMatch(/^#[0-9a-f]{8}$/);
  });
  it("spreads names across the palette", () => {
    const names = ["ada", "grace", "edsger", "barbara", "tony"];
    const distinct = new Set(names.map((name) => presenceColor(name).color));
    expect(distinct.size).toBeGreaterThan(1);
  });
});
