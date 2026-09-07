/**
 * The accent scale is a design token, not a place a "tidy" pass should be
 * free to nudge: this file reads the eleven `--color-accent-*` custom
 * properties straight out of `index.css`, the same source Tailwind's
 * `@theme` block draws from, and holds them to two bars.
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

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

const CSS_PATH = join(__dirname, "index.css");

/** The eleven accent stops, read out of the `@theme` block by name. */
function readAccentScale(): Record<string, string> {
  const css = readFileSync(CSS_PATH, "utf8");
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
