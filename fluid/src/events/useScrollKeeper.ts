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

  useLayoutEffect(() => {
    if (off || !fetching || shown.current == null) return;
    anchor.current = captureAnchor(document, {
      scrollY: window.scrollY,
      innerHeight: window.innerHeight,
    });
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
}
