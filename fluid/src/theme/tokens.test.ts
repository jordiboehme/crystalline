/// <reference types="node" />
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, test } from "vitest";

const css = readFileSync(join(__dirname, "..", "index.css"), "utf8");

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
