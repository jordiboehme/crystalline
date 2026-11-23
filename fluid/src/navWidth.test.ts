import { afterEach, describe, expect, it, vi } from "vitest";

import {
  NAV_WIDTH_DEFAULT,
  NAV_WIDTH_KEY,
  clampNavWidth,
  saveNavWidth,
  settleNavWidth,
  storedNavWidth,
} from "./navWidth";

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("the sidebar width", () => {
  it("clamps to 200 and 480", () => {
    expect(clampNavWidth(100)).toBe(200);
    expect(clampNavWidth(333.4)).toBe(333);
    expect(clampNavWidth(900)).toBe(480);
  });

  it("settles under the minimum as the rail and elsewhere as a width", () => {
    expect(settleNavWidth(199)).toEqual({ kind: "rail" });
    expect(settleNavWidth(200)).toEqual({ kind: "width", width: 200 });
    expect(settleNavWidth(700)).toEqual({ kind: "width", width: 480 });
  });

  it("stores under its own key and reads it back", () => {
    saveNavWidth(320);
    expect(localStorage.getItem(NAV_WIDTH_KEY)).toBe("320");
    expect(NAV_WIDTH_KEY).not.toBe("fluid.nav");
    expect(storedNavWidth()).toBe(320);
  });

  it.each(["", "abc", "199", "481", "300.5", "-300"])(
    "reads %j as the default",
    (value) => {
      localStorage.setItem(NAV_WIDTH_KEY, value);
      expect(storedNavWidth()).toBe(NAV_WIDTH_DEFAULT);
    },
  );

  it("reads the default when nothing is stored", () => {
    expect(storedNavWidth()).toBe(NAV_WIDTH_DEFAULT);
  });

  it("survives a browser that refuses storage", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    expect(storedNavWidth()).toBe(NAV_WIDTH_DEFAULT);
    expect(() => {
      saveNavWidth(300);
    }).not.toThrow();
  });
});
