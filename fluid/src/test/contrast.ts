/**
 * WCAG 2.x contrast for tests: the ratio of two colours given as `#rrggbb`
 * or as the `rgb(r, g, b)` a rendered style reads back as.
 */

/** The three 0 to 255 channels of `#rrggbb` or `rgb(r, g, b)`. */
function channels(colour: string): [number, number, number] {
  const rgb = /^rgb\((\d+),\s*(\d+),\s*(\d+)\)$/.exec(colour.trim());
  if (rgb) return [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])];
  const hex = /^#([0-9a-f]{6})$/i.exec(colour.trim());
  if (!hex) throw new Error(`not a colour: ${colour}`);
  const value = hex[1]!;
  return [0, 2, 4].map((i) => parseInt(value.slice(i, i + 2), 16)) as [
    number,
    number,
    number,
  ];
}

/** Relative luminance, WCAG 2.x. */
function luminance(colour: string): number {
  const [r, g, b] = channels(colour).map((raw) => {
    const c = raw / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  }) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** The contrast ratio of two colours: (L1 + 0.05) / (L2 + 0.05). */
export function contrastRatio(a: string, b: string): number {
  const [la, lb] = [luminance(a), luminance(b)];
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** Every `#rrggbb` a class list names for `prefix`, e.g. `hover:bg-`. */
export function classColours(classes: string, prefix: string): string[] {
  return classes
    .split(/\s+/)
    .filter((c) => c.startsWith(`${prefix}[#`))
    .map((c) => c.slice(prefix.length + 1, -1));
}
