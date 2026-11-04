/** Where an element popover stands, from the element's box. */

/** The gap between the popover and the element, in pixels. */
const GAP = 6;

/**
 * Where the popover goes, relative to its positioned parent `box`: under
 * `rect`, or above it when there is no room below and there is room above
 * (under `ceiling`, the sticky bar's bottom); its left edge at the element's,
 * kept inside the box.
 */
export function placement(
  rect: { top: number; bottom: number; left: number },
  size: { width: number; height: number },
  box: { top: number; left: number; width: number },
  viewportHeight: number,
  ceiling = 0,
): { top: number; left: number } {
  let top = rect.bottom + GAP;
  const above = rect.top - GAP - size.height;
  if (top + size.height > viewportHeight && above >= ceiling) {
    top = above;
  }
  const left = Math.max(
    box.left,
    Math.min(rect.left, box.left + box.width - size.width),
  );
  return { top: top - box.top, left: left - box.left };
}
