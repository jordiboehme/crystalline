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
  useHoldPlace,
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
  /** A heading in the live document whose viewport top the test moves. */
  let heading: HTMLElement | null = null;
  let headingTop = 0;
  function placeHeading(top: number) {
    headingTop = top;
    heading = document.createElement("h2");
    heading.id = "auth";
    heading.getBoundingClientRect = () => rect(headingTop, 20);
    document.body.appendChild(heading);
  }

  afterEach(() => {
    heading?.remove();
    heading = null;
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
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

  it("puts the anchor back where it sat before the new content", () => {
    // Captured over the OLD content: a capture taken after the new content
    // rendered would read the moved heading as the place to keep and never
    // scroll at all.
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    vi.spyOn(window, "scrollY", "get").mockReturnValue(1000);
    placeHeading(-20);
    const hook = mount({
      address: "eng/alpha",
      fetching: false,
      checksum: "a1",
    });
    hook.rerender({ address: "eng/alpha", fetching: true, checksum: "a1" });
    // The new content pushed the heading 200px further down.
    headingTop = 180;
    hook.rerender({ address: "eng/alpha", fetching: false, checksum: "a2" });
    expect(scrollTo).toHaveBeenCalledWith(0, 1200);
  });

  it("keeps the place the reader scrolled to while the fetch was in flight", () => {
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    vi.stubGlobal("requestAnimationFrame", (run: FrameRequestCallback) => {
      run(0);
      return 1;
    });
    vi.spyOn(window, "scrollY", "get").mockReturnValue(1000);
    placeHeading(-20);
    const hook = mount({
      address: "eng/alpha",
      fetching: false,
      checksum: "a1",
    });
    hook.rerender({ address: "eng/alpha", fetching: true, checksum: "a1" });
    // The reader scrolls on while the GET runs: the heading now sits 100px
    // above the top, and that is the offset to keep.
    headingTop = -100;
    window.dispatchEvent(new Event("scroll"));
    headingTop = 180;
    hook.rerender({ address: "eng/alpha", fetching: false, checksum: "a2" });
    expect(scrollTo).toHaveBeenCalledWith(0, 1280);
  });

  it("drops the anchor of a fetch that brought nothing new", () => {
    // A focus refetch that found the same text leaves no anchor behind for
    // a later change that arrives without a fetch of its own.
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    const hook = mount({
      address: "eng/alpha",
      fetching: false,
      checksum: "a1",
    });
    hook.rerender({ address: "eng/alpha", fetching: true, checksum: "a1" });
    hook.rerender({ address: "eng/alpha", fetching: false, checksum: "a1" });
    hook.rerender({ address: "eng/alpha", fetching: false, checksum: "a2" });
    expect(scrollTo).not.toHaveBeenCalled();
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

describe("holding the place while the line comes and goes", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  function mountHold(element: HTMLElement, shape: string) {
    return renderHook(
      (props: { shape: string }) => {
        useHoldPlace(() => element, props.shape);
      },
      { initialProps: { shape } },
    );
  }

  it("scrolls by what the heading moved once the reader is past it", () => {
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    vi.spyOn(window, "scrollY", "get").mockReturnValue(800);
    let top = -300;
    const heading = document.createElement("h2");
    heading.getBoundingClientRect = () => rect(top, 20);
    const hook = mountHold(heading, "");
    // The line appears above and pushes the heading down 60px.
    top = -240;
    hook.rerender({ shape: "Updated a moment ago" });
    expect(scrollTo).toHaveBeenCalledWith(0, 860);
    // The browser scrolled; the heading is back. Then the line goes.
    top = -300;
    window.dispatchEvent(new Event("scroll"));
    top = -360;
    hook.rerender({ shape: "" });
    expect(scrollTo).toHaveBeenLastCalledWith(0, 740);
  });

  it("measures from where the reader scrolled to, not where the page opened", () => {
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    vi.spyOn(window, "scrollY", "get").mockReturnValue(1300);
    let top = -300;
    const heading = document.createElement("h2");
    heading.getBoundingClientRect = () => rect(top, 20);
    const hook = mountHold(heading, "");
    // The reader reads on for a while before anything changes.
    top = -800;
    window.dispatchEvent(new Event("scroll"));
    top = -740;
    hook.rerender({ shape: "Updated a moment ago" });
    expect(scrollTo).toHaveBeenCalledWith(0, 1360);
  });

  it("does nothing where the browser already held it", () => {
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    const heading = document.createElement("h2");
    heading.getBoundingClientRect = () => rect(-300, 20);
    const hook = mountHold(heading, "");
    hook.rerender({ shape: "Updated a moment ago" });
    expect(scrollTo).not.toHaveBeenCalled();
  });

  it("lets the line push the text while the reader can see it arrive", () => {
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    let top = 200;
    const heading = document.createElement("h2");
    heading.getBoundingClientRect = () => rect(top, 20);
    const hook = mountHold(heading, "");
    top = 260;
    hook.rerender({ shape: "Updated a moment ago" });
    expect(scrollTo).not.toHaveBeenCalled();
  });
});
