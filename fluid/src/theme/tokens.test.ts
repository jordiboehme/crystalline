/**
 * The app's design tokens read back out of `index.css` - the source
 * Tailwind's `@theme` block and the syntax-highlighting rules both draw
 * from - so a later edit to the stylesheet is checked against the same
 * bars a reader would check it against by eye.
 *
 * Node-context code (`node:fs`/`node:path`/`__dirname`), not browser code,
 * so it typechecks under `tsconfig.node-test.json` rather than the app's own
 * `tsconfig.app.json` (see that file's exclude comment).
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it, test } from "vitest";

const CSS_PATH = join(__dirname, "..", "index.css");
const css = readFileSync(CSS_PATH, "utf8");
const EDITOR_SETUP_PATH = join(__dirname, "..", "editor", "setup.ts");
const editorSetup = readFileSync(EDITOR_SETUP_PATH, "utf8");

describe("design tokens", () => {
  test("the five-step scale exists and floors at 12px", () => {
    for (const step of ["display", "title", "section", "body", "caption"]) {
      expect(css).toContain(`--text-${step}:`);
    }
    const caption = /--text-caption:\s*([\d.]+)rem/.exec(css);
    expect(caption).not.toBeNull();
    expect(Number(caption?.[1])).toBeGreaterThanOrEqual(0.75);
  });

  test("the accent ramp exists", () => {
    for (const stop of ["400", "600", "700"]) {
      expect(css).toContain(`--color-accent-${stop}:`);
    }
  });

  test("the measure helper caps prose and exempts breakouts", () => {
    expect(css).toContain(".measured > :not(.breakout)");
  });
});

/**
 * The accent scale, WCAG contrast and the two-tone syntax-highlighting
 * palette that shares it.
 *
 * The accent scale is a design token, not a place a "tidy" pass should be
 * free to nudge: this reads the eleven `--color-accent-*` custom properties
 * and holds them to two bars.
 *
 * WCAG contrast, so text and icons drawn in the scale stay legible: the
 * pairs below are the ones the app actually asks the scale to carry (the
 * light-mode workhorse tones on white, the dark-mode workhorse tones on the
 * app's own slate-950, and the mid stop wherever it is used for icons,
 * borders or large text rather than body copy).
 *
 * And the four values the Crystalline Handbook itself uses - 200, 500, 800
 * and 950 - present verbatim, so a later retune cannot drift Fluid's accent
 * away from the book's without this test noticing.
 */

/** The eleven accent stops, read out of the `@theme` block by name. */
function readAccentScale(): Record<string, string> {
  const scale: Record<string, string> = {};
  const pattern = /--color-accent-(\d+):\s*(#[0-9a-fA-F]{6})\s*;/g;
  for (const match of css.matchAll(pattern)) {
    const [, step, hex] = match;
    if (step && hex) {
      scale[step] = hex.toLowerCase();
    }
  }
  return scale;
}

/**
 * The value a custom property resolves to under one `data-theme` block.
 *
 * `[data-theme="light"]`/`[data-theme="dark"]` each appear exactly once in
 * `index.css` as a bare selector (immediately followed by `{`, no descendant
 * class) - the `color-scheme` block in `@layer base` - which is what makes
 * this safe to find by that anchor rather than by document order.
 */
function themeBlock(theme: "light" | "dark"): string {
  const pattern = new RegExp(`\\[data-theme="${theme}"\\]\\s*{([^}]*)}`);
  const match = pattern.exec(css);
  if (!match?.[1]) {
    throw new Error(`no [data-theme="${theme}"] { ... } block in index.css`);
  }
  return match[1];
}

function cssVar(block: string, name: string): string {
  const pattern = new RegExp(`${name}:\\s*(#[0-9a-fA-F]{6})`);
  const match = pattern.exec(block);
  if (!match?.[1]) {
    throw new Error(`${name} not found`);
  }
  return match[1].toLowerCase();
}

/** The first `color:` declaration after the given anchor text. */
function colorAfter(anchor: string): string {
  const at = css.indexOf(anchor);
  if (at === -1) {
    throw new Error(`anchor not found: ${anchor}`);
  }
  const match = /color:\s*(#[0-9a-fA-F]{6})/.exec(css.slice(at));
  if (!match?.[1]) {
    throw new Error(`no color found after: ${anchor}`);
  }
  return match[1].toLowerCase();
}

/** sRGB hex to relative luminance, WCAG 2.x. */
function relativeLuminance(hex: string): number {
  const value = hex.replace("#", "");
  const channels = [0, 2, 4].map((offset) => {
    const raw = parseInt(value.slice(offset, offset + 2), 16) / 255;
    return raw <= 0.03928 ? raw / 12.92 : ((raw + 0.055) / 1.055) ** 2.4;
  });
  const [r, g, b] = channels as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** Contrast ratio between two colors, WCAG 2.x: (L1 + 0.05) / (L2 + 0.05). */
function contrastRatio(a: string, b: string): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  const lighter = Math.max(la, lb);
  const darker = Math.min(la, lb);
  return (lighter + 0.05) / (darker + 0.05);
}

/** sRGB hex to CIE L*a*b*, D65 white point. */
function toLab(hex: string): [number, number, number] {
  const value = hex.replace("#", "");
  const srgb = [0, 2, 4].map((offset) => {
    const raw = parseInt(value.slice(offset, offset + 2), 16) / 255;
    return raw <= 0.04045 ? raw / 12.92 : ((raw + 0.055) / 1.055) ** 2.4;
  });
  const [r, g, b] = srgb as [number, number, number];
  const x = r * 0.4124564 + g * 0.3575761 + b * 0.1804375;
  const y = r * 0.2126729 + g * 0.7151522 + b * 0.072175;
  const z = r * 0.0193339 + g * 0.119192 + b * 0.9503041;
  const [xn, yn, zn] = [0.95047, 1.0, 1.08883];
  const f = (t: number) => (t > 0.008856 ? Math.cbrt(t) : 7.787 * t + 16 / 116);
  const [fx, fy, fz] = [f(x / xn), f(y / yn), f(z / zn)];
  return [116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz)];
}

/** CIE76 color difference: perceptual distance, not just a hex diff. */
function deltaE(a: string, b: string): number {
  const [l1, a1, b1] = toLab(a);
  const [l2, a2, b2] = toLab(b);
  return Math.sqrt((l1 - l2) ** 2 + (a1 - a2) ** 2 + (b1 - b2) ** 2);
}

const WHITE = "#ffffff";
const SLATE_950 = "#020617";

describe("the accent scale", () => {
  const scale = readAccentScale();

  it("carries all eleven stops", () => {
    expect(Object.keys(scale).sort()).toEqual(
      [
        "50",
        "100",
        "200",
        "300",
        "400",
        "500",
        "600",
        "700",
        "800",
        "900",
        "950",
      ].sort(),
    );
  });

  it("holds the four values the handbook itself uses, verbatim", () => {
    // 200 and 950 bound the scale's cool ends, 500 is the light C64 blue and
    // 800 the dark one - the four stops a later retune must not drift.
    expect(scale["200"]).toBe("#d9d3f4");
    expect(scale["500"]).toBe("#6c5eb5");
    expect(scale["800"]).toBe("#352879");
    expect(scale["950"]).toBe("#1d1739");
  });

  it.each(["600", "700", "800"])(
    "clears 4.5:1 for accent-%s text on white, the light-mode body pairing",
    (step) => {
      const hex = scale[step];
      expect(hex).toBeDefined();
      expect(contrastRatio(hex as string, WHITE)).toBeGreaterThanOrEqual(4.5);
    },
  );

  it.each(["300", "400"])(
    "clears 4.5:1 for accent-%s text on slate-950, the dark-mode body pairing",
    (step) => {
      const hex = scale[step];
      expect(hex).toBeDefined();
      expect(contrastRatio(hex as string, SLATE_950)).toBeGreaterThanOrEqual(
        4.5,
      );
    },
  );

  it("clears 3:1 for accent-500 against both backgrounds (icons, borders, large text)", () => {
    const hex = scale["500"];
    expect(hex).toBeDefined();
    expect(contrastRatio(hex as string, SLATE_950)).toBeGreaterThanOrEqual(3);
    expect(contrastRatio(hex as string, WHITE)).toBeGreaterThanOrEqual(3);
  });
});

/**
 * The `--color-code-string` custom property CodeMirror's `tags.string` rule
 * reads (`editor/setup.ts`), scheme by scheme - the fix for a review finding
 * that the editor's string color was one static value in both themes, which
 * fell to 2.11:1 on the dark editor's own background. Each scheme's value is
 * pinned to what `.hljs-string` uses for that scheme, which is what "a
 * fenced code block does not change hue between editing and reading" means
 * in practice, and each is held to the same 4.5:1 body-text bar the accent
 * scale itself is held to above.
 */
describe("the editor's code-string color, per scheme", () => {
  it("matches .hljs-string in light mode and clears 4.5:1 on white", () => {
    const light = cssVar(themeBlock("light"), "--color-code-string");
    expect(light).toBe(colorAfter(".hljs-string,"));
    expect(contrastRatio(light, WHITE)).toBeGreaterThanOrEqual(4.5);
  });

  it("matches .hljs-string in dark mode and clears 4.5:1 on slate-950", () => {
    const dark = cssVar(themeBlock("dark"), "--color-code-string");
    expect(dark).toBe(colorAfter('[data-theme="dark"] .hljs-string,'));
    expect(contrastRatio(dark, SLATE_950)).toBeGreaterThanOrEqual(4.5);
  });

  // Both tests above pin a property nothing here reads. What makes them worth
  // anything is that the editor still asks for it: a `tags.string` rule
  // rewritten back to a literal color would leave two green tests and a
  // regression, since the per-scheme values would simply stop being consulted.
  it("is what the editor's string rule actually reads", () => {
    expect(editorSetup).toMatch(
      /tag:\s*tags\.string\s*,\s*color:\s*"var\(--color-code-string\)"/,
    );
  });
});

/**
 * The dark reading-view syntax palette needs enough separation between
 * tokens to read as different colors, not contrast against the page: a
 * review found the dark `.hljs-title`/`.hljs-type`/`.hljs-built_in` rule at
 * `#d8b4fe` sat only 14.0 CIE76 units from `.hljs-string`'s `#b9afe6` -
 * indistinguishable in 13px mono - while light mode was never at risk (32-46
 * units apart). The floor below is 18, comfortably under the fixed palette's
 * actual minimum (18.6, string vs number) and comfortably over the
 * regression's 14.0, so a color nudged back toward the accent trips it.
 */
describe("the dark syntax-highlighting palette keeps its tokens apart", () => {
  const MIN_SEPARATION = 18;

  it("holds every pair of {string, title, number, comment} at or above the floor", () => {
    const swatches: Record<string, string> = {
      comment: colorAfter('[data-theme="dark"] .hljs-comment,'),
      string: colorAfter('[data-theme="dark"] .hljs-string,'),
      number: colorAfter('[data-theme="dark"] .hljs-number,'),
      title: colorAfter('[data-theme="dark"] .hljs-title,'),
    };
    const names = Object.keys(swatches);
    for (let i = 0; i < names.length; i += 1) {
      for (let j = i + 1; j < names.length; j += 1) {
        const [nameA, nameB] = [names[i] as string, names[j] as string];
        const [hexA, hexB] = [
          swatches[nameA] as string,
          swatches[nameB] as string,
        ];
        expect(
          deltaE(hexA, hexB),
          `${nameA} (${hexA}) vs ${nameB} (${hexB})`,
        ).toBeGreaterThanOrEqual(MIN_SEPARATION);
      }
    }
  });
});
