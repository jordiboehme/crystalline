import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { usePageTitle } from "./usePageTitle";

afterEach(() => {
  document.title = "Fluid";
});

describe("usePageTitle", () => {
  it("names the page and puts the app name after it", () => {
    renderHook(() => {
      usePageTitle("Berth assignment");
    });
    expect(document.title).toBe("Berth assignment - Fluid");
  });

  it("follows a title that changes while the page is open", () => {
    const { rerender } = renderHook(
      ({ title }: { title: string }) => {
        usePageTitle(title);
      },
      { initialProps: { title: "One" } },
    );
    rerender({ title: "Two" });
    expect(document.title).toBe("Two - Fluid");
  });

  it("goes back to Fluid when the page leaves", () => {
    const { unmount } = renderHook(() => {
      usePageTitle("Gone soon");
    });
    unmount();
    expect(document.title).toBe("Fluid");
  });

  it("says Fluid while there is no title, never undefined", () => {
    const { rerender } = renderHook(
      ({ title }: { title: string | null | undefined }) => {
        usePageTitle(title);
      },
      { initialProps: { title: undefined as string | null | undefined } },
    );
    expect(document.title).toBe("Fluid");
    rerender({ title: "   " });
    expect(document.title).toBe("Fluid");
    rerender({ title: "Loaded" });
    expect(document.title).toBe("Loaded - Fluid");
    rerender({ title: null });
    expect(document.title).toBe("Fluid");
  });

  it("is only the bare title while printing and goes back after", () => {
    renderHook(() => {
      usePageTitle("Berth assignment");
    });
    act(() => {
      window.dispatchEvent(new Event("beforeprint"));
    });
    expect(document.title).toBe("Berth assignment");
    act(() => {
      window.dispatchEvent(new Event("afterprint"));
    });
    expect(document.title).toBe("Berth assignment - Fluid");
  });

  it("prints under the print title when it is given", () => {
    renderHook(() => {
      usePageTitle("Welcome", "Welcome page");
    });
    act(() => {
      window.dispatchEvent(new Event("beforeprint"));
    });
    expect(document.title).toBe("Welcome page");
  });

  it("keeps Fluid while printing a page without a title", () => {
    renderHook(() => {
      usePageTitle(null);
    });
    act(() => {
      window.dispatchEvent(new Event("beforeprint"));
    });
    expect(document.title).toBe("Fluid");
  });

  it("removes its listeners when the page leaves", () => {
    const add = vi.spyOn(window, "addEventListener");
    const remove = vi.spyOn(window, "removeEventListener");
    const { unmount } = renderHook(() => {
      usePageTitle("Listening");
    });
    unmount();
    for (const type of ["beforeprint", "afterprint"]) {
      const added = add.mock.calls.filter((call) => call[0] === type);
      const removed = remove.mock.calls.filter((call) => call[0] === type);
      expect(added).toHaveLength(1);
      expect(removed).toHaveLength(1);
      expect(removed[0]?.[1]).toBe(added[0]?.[1]);
    }
    act(() => {
      window.dispatchEvent(new Event("beforeprint"));
    });
    expect(document.title).toBe("Fluid");
    add.mockRestore();
    remove.mockRestore();
  });

  it("keeps a title with slashes and quotes as the text it is", () => {
    renderHook(() => {
      usePageTitle(`Plan A/B "final" it's`);
    });
    expect(document.title).toBe(`Plan A/B "final" it's - Fluid`);
  });
});
