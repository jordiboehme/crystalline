/**
 * The HUD's prompt line stays readable whatever lies behind it (2.6e C28):
 * over the white doorway of an open police box as over a dark wall. And the
 * frame time is a development aid, drawn only in a development build.
 */

import { render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Hud, PROMPT_BACKING } from "./Hud";
import type { HudView } from "./useHud";

/** An sRGB channel in [0, 1], linearised (WCAG 2's relative luminance). */
const linear = (c: number) =>
  c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;

/** The contrast ratio of two grey levels in [0, 1] (WCAG 2). */
const contrast = (a: number, b: number) => {
  const [hi, lo] = [linear(a), linear(b)].sort((x, y) => y - x);
  return ((hi ?? 0) + 0.05) / ((lo ?? 0) + 0.05);
};

/** The HUD mounted with no-op sinks, and its prompt element. */
function mountedPrompt(): HTMLElement {
  let prompt: HTMLElement | null = null;
  const view: HudView = {
    prompt: (el) => {
      prompt = el;
    },
    status: () => undefined,
    frame: () => undefined,
    notice: () => undefined,
  };
  render(<Hud view={view} connector={{ active: false, label: "" }} />);
  if (prompt === null) throw new Error("no prompt element");
  return prompt;
}

describe("the HUD's prompt (2.6e C28)", () => {
  it("sits on a dark backing that keeps its white text at 4.5:1 over a white doorway", () => {
    // Mutation caught: the backing dropped, made too light to read white
    // on white (under 60 % black), or out of step with the class the
    // element carries.
    const el = mountedPrompt();
    const alpha = /(?:^|\s)bg-black\/(\d+)(?:\s|$)/.exec(el.className);
    expect(alpha, el.className).not.toBeNull();
    expect(Number(alpha?.[1]) / 100).toBe(PROMPT_BACKING);
    // White text over the backing laid on the brightest thing behind it:
    // a white doorway (1.0) seen through black at `PROMPT_BACKING`.
    const behind = 1 - PROMPT_BACKING;
    expect(contrast(1, behind)).toBeGreaterThanOrEqual(4.5);
    expect(el.className).toMatch(/(?:^|\s)text-white(?:\s|$)/);
  });

  it("fits its backing to the prompt, not a bar across the screen", () => {
    // Mutation caught: the backing put on a full-width line (`inset-x-0`),
    // which paints a dark bar over the bottom of the view.
    const el = mountedPrompt();
    expect(el.className).not.toMatch(/(?:^|\s)inset-x-0(?:\s|$)/);
    expect(el.className).toMatch(/(?:^|\s)left-1\/2(?:\s|$)/);
    expect(el.className).toMatch(/(?:^|\s)-translate-x-1\/2(?:\s|$)/);
    expect(el.hidden).toBe(true);
  });
});

describe("the HUD's frame line", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
  });

  /** The element the frame line's ref was bound to, or null for none. */
  function mountedFrame(): HTMLElement | null {
    let frame: HTMLElement | null = null;
    const view: HudView = {
      prompt: () => undefined,
      status: () => undefined,
      frame: (el) => {
        frame = el;
      },
      notice: () => undefined,
    };
    render(<Hud view={view} connector={{ active: false, label: "" }} />);
    return frame;
  }

  it("shows the frame line only in development", () => {
    // Mutation caught: the gate dropped (the frame time drawn in a
    // production build), or inverted (never drawn in development).
    vi.stubEnv("DEV", true);
    expect(mountedFrame()).toBeInstanceOf(HTMLElement);
    vi.stubEnv("DEV", false);
    expect(mountedFrame()).toBeNull();
  });
});
