/**
 * Keep the reader's place when the page under them is refetched.
 *
 * Best effort by agreement: the anchor is the nearest heading with an id at
 * or above the viewport top (wave 2's heading slugs give every heading one),
 * then any element with an id, then the old scroll offset clamped to the new
 * height. A heading renamed in the change counts as gone and the fallback
 * runs. The scroll is instant, so `prefers-reduced-motion` does not apply,
 * and the keeper is inert in the print view.
 *
 * The capture runs when the detail query starts refetching, while the DOM is
 * still the old content; the restore runs in a layout effect once the
 * checksum changed, before the browser paints the new one.
 *
 * Only a new value of the same engram counts. The reading page stays mounted
 * when the reader follows a link to another engram, so the keeper is told the
 * address it is keeping: when that changes, what it held belongs to the page
 * the reader left and is dropped, and the new page opens where the browser
 * puts it.
 */

import { useLayoutEffect, useRef } from "react";

export interface ScrollAnchor {
  id: string | null;
  /** The anchor's `getBoundingClientRect().top` at capture. */
  top: number;
  scrollY: number;
}

export function captureAnchor(
  root: ParentNode,
  view: { scrollY: number; innerHeight: number },
): ScrollAnchor {
  const pick = (selector: string): { id: string; top: number } | null => {
    let best: { id: string; top: number } | null = null;
    for (const element of root.querySelectorAll<HTMLElement>(selector)) {
      if (!element.id) continue;
      const top = element.getBoundingClientRect().top;
      if (top <= 0 && (best === null || top > best.top)) {
        best = { id: element.id, top };
      }
    }
    return best;
  };
  const found =
    pick("h1[id],h2[id],h3[id],h4[id],h5[id],h6[id]") ?? pick("[id]");
  return { id: found?.id ?? null, top: found?.top ?? 0, scrollY: view.scrollY };
}

/** The element carrying `id` under `root`, without building a selector from it. */
function byId(root: ParentNode, id: string): HTMLElement | null {
  for (const element of root.querySelectorAll<HTMLElement>("[id]")) {
    if (element.id === id) return element;
  }
  return null;
}

export function restoreAnchor(
  anchor: ScrollAnchor,
  root: ParentNode,
  view: {
    scrollTo(x: number, y: number): void;
    scrollY: number;
    documentHeight: number;
    innerHeight: number;
  },
): void {
  const target = anchor.id ? byId(root, anchor.id) : null;
  if (target) {
    const top = target.getBoundingClientRect().top;
    view.scrollTo(0, view.scrollY + top - anchor.top);
    return;
  }
  const max = Math.max(0, view.documentHeight - view.innerHeight);
  view.scrollTo(0, Math.min(anchor.scrollY, max));
}

export function useScrollKeeper({
  address,
  fetching,
  checksum,
  disabled = false,
}: {
  /** Which engram the page shows; a new one starts the keeper afresh. */
  address: string;
  fetching: boolean;
  checksum: string | null | undefined;
  disabled?: boolean;
}): void {
  const anchor = useRef<ScrollAnchor | null>(null);
  const shown = useRef<string | null | undefined>(checksum);
  const kept = useRef(address);
  const printing =
    typeof window.matchMedia === "function" &&
    window.matchMedia("print").matches;
  const off = disabled || printing;

  // Declared first so it runs first: a new address forgets the old page's
  // checksum and anchor before either effect below reads them.
  useLayoutEffect(() => {
    if (kept.current === address) return;
    kept.current = address;
    shown.current = undefined;
    anchor.current = null;
  }, [address]);

  // Captured when the refetch starts and again whenever the reader scrolls
  // while it is in flight (once per frame at most): on a slow link the fetch
  // takes long enough to scroll away from where it started, and the place to
  // hold is the last one before the new content lands.
  useLayoutEffect(() => {
    if (off || !fetching || shown.current == null) return;
    const capture = () => {
      anchor.current = captureAnchor(document, {
        scrollY: window.scrollY,
        innerHeight: window.innerHeight,
      });
    };
    capture();
    let frame: number | null = null;
    const onScroll = () => {
      frame ??= requestAnimationFrame(() => {
        frame = null;
        capture();
      });
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      window.removeEventListener("scroll", onScroll);
      if (frame !== null) cancelAnimationFrame(frame);
    };
  }, [fetching, off]);

  useLayoutEffect(() => {
    if (off || checksum == null) return;
    const previous = shown.current;
    shown.current = checksum;
    if (previous == null || previous === checksum || !anchor.current) return;
    restoreAnchor(anchor.current, document, {
      scrollTo: (x, y) => {
        window.scrollTo(x, y);
      },
      scrollY: window.scrollY,
      documentHeight: document.documentElement.scrollHeight,
      innerHeight: window.innerHeight,
    });
    anchor.current = null;
  }, [checksum, off]);

  // Declared last so it runs after the restore: a fetch that ended without a
  // new checksum (a focus refetch that found nothing new, a cancelled one)
  // drops its anchor, so a later change that arrives some other way is never
  // restored against a place the reader left long ago.
  useLayoutEffect(() => {
    if (!fetching) anchor.current = null;
  }, [fetching]);
}

/**
 * Hold the reader's place while something above it changes height.
 *
 * The status line above the body appears when a change arrives and goes a
 * minute later. A browser with native scroll anchoring absorbs both shifts;
 * one without it moves the text under the reader, twice. `below` names the
 * first element under the thing that changes, and `shape` is what changes it:
 * when `shape` changes and that element had scrolled past the viewport top,
 * the page scrolls by exactly as much as the element moved. Where the browser
 * already compensated the element has not moved, and this does nothing.
 *
 * Where the reader can see the element, nothing is held: the line appearing
 * in view is meant to be seen.
 */
export function useHoldPlace(
  below: () => Element | null,
  shape: unknown,
): void {
  const last = useRef<number | null>(null);
  const target = useRef(below);
  useLayoutEffect(() => {
    target.current = below;
  });

  // Where the element stood, kept current as the reader scrolls.
  useLayoutEffect(() => {
    const measure = () => {
      last.current = target.current()?.getBoundingClientRect().top ?? null;
    };
    measure();
    window.addEventListener("scroll", measure, { passive: true });
    window.addEventListener("resize", measure);
    return () => {
      window.removeEventListener("scroll", measure);
      window.removeEventListener("resize", measure);
    };
  }, []);

  useLayoutEffect(() => {
    const element = target.current();
    const before = last.current;
    const now = element?.getBoundingClientRect().top ?? null;
    last.current = now;
    if (now === null || before === null || before >= 0 || now === before) {
      return;
    }
    window.scrollTo(0, window.scrollY + now - before);
    last.current = before;
  }, [shape]);
}
