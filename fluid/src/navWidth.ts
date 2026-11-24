/**
 * How wide the domain sidebar is, and what a drag of its divider means.
 *
 * The width is a choice about the shape of the frame, kept the way the fold
 * is (`fluid.nav` in `Layout.tsx`) but under a key of its own, in this
 * browser only. A rail is not a width: folding keeps the width the sidebar
 * had, so unfolding gives it back.
 */

/** Where the width is remembered, beside `fluid.nav`. */
export const NAV_WIDTH_KEY = "fluid.navWidth";

/** The narrowest the sidebar goes before a drag folds it to the rail. */
export const NAV_WIDTH_MIN = 200;
/** The widest a drag makes it. */
export const NAV_WIDTH_MAX = 480;
/** What it was before it could be dragged (Tailwind's `w-64`). */
export const NAV_WIDTH_DEFAULT = 256;

/** The nearest width inside the allowed range. */
export function clampNavWidth(width: number): number {
  return Math.min(NAV_WIDTH_MAX, Math.max(NAV_WIDTH_MIN, Math.round(width)));
}

/**
 * What letting go at `width` means: a width in range, or the rail when the
 * pointer went under the minimum.
 */
export function settleNavWidth(
  width: number,
): { kind: "width"; width: number } | { kind: "rail" } {
  return width < NAV_WIDTH_MIN
    ? { kind: "rail" }
    : { kind: "width", width: clampNavWidth(width) };
}

/**
 * The width last left, read once at mount.
 *
 * A browser that refuses storage is not a reason to fail to draw the frame,
 * and neither is a value that is not a number in range: both read as the
 * default.
 */
export function storedNavWidth(): number {
  try {
    const raw = localStorage.getItem(NAV_WIDTH_KEY);
    if (raw === null || !/^\d+$/.test(raw)) {
      return NAV_WIDTH_DEFAULT;
    }
    const width = Number(raw);
    return width >= NAV_WIDTH_MIN && width <= NAV_WIDTH_MAX
      ? width
      : NAV_WIDTH_DEFAULT;
  } catch {
    return NAV_WIDTH_DEFAULT;
  }
}

/** Remember a width. The session keeps it even when storage refuses it. */
export function saveNavWidth(width: number): void {
  try {
    localStorage.setItem(NAV_WIDTH_KEY, String(width));
  } catch {
    // A browser that refuses storage still gets the session's choice.
  }
}
