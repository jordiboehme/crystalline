/**
 * The scroll keeper, over a fake layout: jsdom lays nothing out, so every
 * heading's `getBoundingClientRect` is stubbed to a known document top minus
 * the scroll offset, which is what a browser would answer.
 */

import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  captureAnchor,
  restoreAnchor,
  useScrollKeeper,
} from "./useScrollKeeper";

function rect(top: number, height: number): DOMRect {
  return {
    top,
    bottom: top + height,
    left: 0,
    right: 0,
    width: 0,
    height,
    x: 0,
    y: top,
    toJSON: () => ({}),
  };
}

/** A document of headings at known tops, with `getBoundingClientRect` stubbed. */
function layout(ids: [string, number][], scrollY: number) {
  const root = document.createElement("div");
  for (const [id, top] of ids) {
    const heading = document.createElement("h2");
    heading.id = id;
    heading.getBoundingClientRect = () => rect(top - scrollY, 20);
    root.appendChild(heading);
  }
  return root;
}

describe("the scroll keeper", () => {
  it("anchors on the nearest heading at or above the viewport top and keeps its offset", () => {
    const before = layout(
      [
        ["intro", 100],
        ["auth", 500],
        ["next", 900],
      ],
      520,
    );
    const anchor = captureAnchor(before, { scrollY: 520, innerHeight: 600 });
    expect(anchor).toEqual({ id: "auth", top: -20, scrollY: 520 });
    // After the refetch the heading sits 200px lower in the document.
    const after = layout(
      [
        ["intro", 100],
        ["auth", 700],
        ["next", 1100],
      ],
      520,
    );
    const calls: [number, number][] = [];
    restoreAnchor(anchor, after, {
      scrollTo: (x, y) => calls.push([x, y]),
      scrollY: 520,
      documentHeight: 2000,
      innerHeight: 600,
    });
    expect(calls).toEqual([[0, 720]]);
  });

  it("falls back to the clamped offset when the anchor vanished", () => {
    const before = layout([["auth", 500]], 520);
    const anchor = captureAnchor(before, { scrollY: 520, innerHeight: 600 });
    const after = layout([["other", 100]], 520);
    const calls: [number, number][] = [];
    restoreAnchor(anchor, after, {
      scrollTo: (x, y) => calls.push([x, y]),
      scrollY: 0,
      documentHeight: 700,
      innerHeight: 600,
    });
    expect(calls, "clamped to what the new height allows").toEqual([[0, 100]]);
  });

  it("counts a heading exactly at the viewport top as above it", () => {
    const root = layout(
      [
        ["intro", 100],
        ["auth", 500],
      ],
      500,
    );
    expect(captureAnchor(root, { scrollY: 500, innerHeight: 600 }).id).toBe(
      "auth",
    );
  });

  it("takes any element with an id when no heading is above the top", () => {
    const root = layout([["late", 900]], 300);
    const figure = document.createElement("figure");
    figure.id = "chart";
    figure.getBoundingClientRect = () => rect(-50, 50);
    root.prepend(figure);
    expect(captureAnchor(root, { scrollY: 300, innerHeight: 600 }).id).toBe(
      "chart",
    );
  });

  it("finds an anchor whose id a CSS selector could not spell", () => {
    // Heading slugs come from prose: a leading digit or a dot is an id a
    // browser accepts and a bare `#id` selector does not.
    const before = layout([["1.2-setup", 400]], 420);
    const anchor = captureAnchor(before, { scrollY: 420, innerHeight: 600 });
    const after = layout([["1.2-setup", 450]], 420);
    const calls: [number, number][] = [];
    restoreAnchor(anchor, after, {
      scrollTo: (x, y) => calls.push([x, y]),
      scrollY: 420,
      documentHeight: 2000,
      innerHeight: 600,
    });
    expect(calls).toEqual([[0, 470]]);
  });
});

describe("the keeper hook", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  type Props = { address: string; fetching: boolean; checksum: string };

  function mount(initial: Props) {
    return renderHook(
      (props: Props) => {
        useScrollKeeper(props);
      },
      { initialProps: initial },
    );
  }

  it("restores after a refetch of the same engram brings a new checksum", () => {
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    const hook = mount({
      address: "eng/alpha",
      fetching: false,
      checksum: "a1",
    });
    hook.rerender({ address: "eng/alpha", fetching: true, checksum: "a1" });
    hook.rerender({ address: "eng/alpha", fetching: false, checksum: "a2" });
    expect(scrollTo).toHaveBeenCalledTimes(1);
  });

  it("leaves the scroll alone when the reader went to another engram", () => {
    // The page stays mounted across a link to another engram: without the
    // address the keeper would carry the old page's offset onto the new one.
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    const hook = mount({
      address: "eng/alpha",
      fetching: false,
      checksum: "a1",
    });
    hook.rerender({ address: "eng/alpha", fetching: true, checksum: "a1" });
    hook.rerender({ address: "eng/beta", fetching: false, checksum: "b1" });
    expect(scrollTo).not.toHaveBeenCalled();
  });

  it("does nothing while disabled", () => {
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    const hook = renderHook(
      (props: Props) => {
        useScrollKeeper({ ...props, disabled: true });
      },
      {
        initialProps: { address: "eng/alpha", fetching: false, checksum: "a1" },
      },
    );
    hook.rerender({ address: "eng/alpha", fetching: true, checksum: "a1" });
    hook.rerender({ address: "eng/alpha", fetching: false, checksum: "a2" });
    expect(scrollTo).not.toHaveBeenCalled();
  });
});
