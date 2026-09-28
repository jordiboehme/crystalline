/**
 * The lift overlay's pure part: the rows built from a lift's stops, in
 * order, each keeping its place in the array.
 */
import { describe, expect, it } from "vitest";

import type { LiftStop } from "../world/types";
import { liftRows } from "./lift";

/** One stop, with every field but the label defaulted. */
function stop(label: string, key = false, here = false): LiftStop {
  return { label, to: { kind: "airlock" }, key, here };
}

describe("liftRows", () => {
  it("keeps the lift's own order, unsorted", () => {
    const rows = liftRows([stop("Zed"), stop("Alpha"), stop("Mid")]);
    expect(rows.map((r) => r.label)).toEqual(["Zed", "Alpha", "Mid"]);
  });

  it("carries each stop's own index in the array", () => {
    const rows = liftRows([stop("a"), stop("b"), stop("c")]);
    expect(rows.map((r) => r.index)).toEqual([0, 1, 2]);
  });

  it("carries the key and here flags across", () => {
    const rows = liftRows([
      stop("a", false, false),
      stop("b", true, false),
      stop("c", false, true),
    ]);
    expect(rows.map((r) => r.key)).toEqual([false, true, false]);
    expect(rows.map((r) => r.here)).toEqual([false, false, true]);
  });

  it("an empty lift: no rows", () => {
    expect(liftRows([])).toEqual([]);
  });
});
