/**
 * The width preference on its own: what a stored value means, and what a
 * window that refuses storage is given instead.
 *
 * The reader is the whole of it. Everything else about full width - the
 * button, the key, the attribute on the frame - is the frame's, and is pinned
 * where the frame is.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

import { LAYOUT_WIDTH_KEY, storedFullWidth } from "./layoutWidth";

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("the stored width", () => {
  it("reads the one value that asks for the whole window", () => {
    localStorage.setItem(LAYOUT_WIDTH_KEY, "full");

    expect(storedFullWidth()).toBe(true);
  });

  it("reads everything else as the reading measure", () => {
    // Nothing stored at all is the default, and so is the word for it; a
    // value from some other version of this app is not a third state.
    expect(storedFullWidth()).toBe(false);
    localStorage.setItem(LAYOUT_WIDTH_KEY, "reading");
    expect(storedFullWidth()).toBe(false);
    localStorage.setItem(LAYOUT_WIDTH_KEY, "wide");
    expect(storedFullWidth()).toBe(false);
  });

  it("still answers where storage is refused", () => {
    // A private window with cookies off, an embedded view: reading the
    // preference throws, and that is not a reason to fail to draw the frame.
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("this window keeps nothing");
    });

    expect(storedFullWidth()).toBe(false);
  });
});
