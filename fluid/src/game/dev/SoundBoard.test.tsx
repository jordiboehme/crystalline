import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AMBIENCES } from "../audio/ambience";
import { BUS_LEVELS } from "../audio/mixer";
import { FakeAudioContext, type FakeNode } from "../audio/testContext";
import SoundBoard from "./SoundBoard";
import { SOUNDS, busOf } from "./sounds";

/** Every context the board made. */
const contexts: FakeAudioContext[] = [];

function stubAudio() {
  vi.stubGlobal(
    "AudioContext",
    class extends FakeAudioContext {
      constructor() {
        super();
        contexts.push(this);
      }
    },
  );
}

/** The bus gain at `level`: the mixer builds one per bus at its level. */
function busAt(ctx: FakeAudioContext, level: number): FakeNode {
  const found = ctx.ofKind("gain").find((g) => g.gain.value === level);
  if (found === undefined) throw new Error(`no bus at ${level}`);
  return found;
}

/** The patch levels feeding `node`: one per patch played into it. */
const into = (ctx: FakeAudioContext, node: FakeNode) =>
  ctx.ofKind("gain").filter((g) => g.connections.includes(node));

afterEach(() => {
  vi.unstubAllGlobals();
  contexts.length = 0;
});

describe("SOUNDS", () => {
  // Mutation caught: a door, a direction or a room kind left off the board
  // (never heard before the ear check).
  it("lists every door both ways, every effect and every drone", () => {
    const names = Object.keys(SOUNDS);
    for (const sound of ["sliding", "bulkhead", "blast", "exit", "box"]) {
      expect(names).toContain(`door ${sound} open`);
      expect(names).toContain(`door ${sound} close`);
    }
    for (const kind of AMBIENCES) expect(names).toContain(`drone ${kind}`);
    for (const name of [
      "step walk",
      "step run",
      "hatch",
      "portal",
      "terminal",
      "fault door",
      "fault hatch",
      "fault portal",
      "ride depart",
      "ride arrive",
    ]) {
      expect(names).toContain(name);
    }
    for (const [name, make] of Object.entries(SOUNDS)) {
      expect(make().voices.length, name).toBeGreaterThan(0);
      expect(busOf(make()), name).toBe(
        name.startsWith("drone") ? "ambience" : "effects",
      );
    }
  });
});

describe("SoundBoard", () => {
  // Mutation caught: a button that plays nothing, a drone on the effects
  // bus, a loop with no way to stop it, or the board's context left open.
  it("plays each patch on its bus, toggles a loop and closes on unmount", () => {
    stubAudio();
    const view = render(<SoundBoard />);
    expect(
      screen.getByRole("heading", { name: "SOUND BOARD" }),
    ).toBeInTheDocument();
    expect(
      screen.getAllByRole("button", { name: /^(door|step|drone)/ }),
    ).toHaveLength(
      Object.keys(SOUNDS).filter((n) => /^(door|step|drone)/.test(n)).length,
    );
    expect(contexts).toHaveLength(0);

    fireEvent.click(screen.getByRole("button", { name: "door sliding open" }));
    expect(contexts).toHaveLength(1);
    const ctx = contexts[0]!;
    expect(ctx.calls).toEqual(["resume"]);
    expect(into(ctx, busAt(ctx, BUS_LEVELS.effects))).toHaveLength(1);

    const drone = screen.getByRole("button", { name: "drone hangar" });
    expect(drone).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(drone);
    expect(drone).toHaveAttribute("aria-pressed", "true");
    const [level] = into(ctx, busAt(ctx, BUS_LEVELS.ambience));
    expect(level).toBeDefined();
    fireEvent.click(drone);
    expect(drone).toHaveAttribute("aria-pressed", "false");
    expect(
      level!.gain.events.some(([m]) => m === "cancelScheduledValues"),
    ).toBe(true);

    view.unmount();
    expect(ctx.calls.at(-1)).toBe("close");
  });
});
