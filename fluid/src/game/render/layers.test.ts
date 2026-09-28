import { describe, expect, it } from "vitest";

import { CANNED_BRIDGE, CANNED_HUB, liftsHallRoom } from "../world/canned";
import { generateRoom } from "../world/generate";
import { LIFT_LINES, LIFT_WORDS } from "../world/lifts";
import type { Fixture, LiftStop, RoomSpec } from "../world/types";
import {
  KEY_MARK,
  LABEL_ROWS,
  LAYER,
  LAYER_SIZE,
  ROW_HEIGHT,
  TEXT_BASE,
  layerPlan,
  textRequests,
} from "./layers";

const bridge = generateRoom(CANNED_BRIDGE);
const hub = generateRoom(CANNED_HUB);

/** Every text key a room's fixtures name, counted from the fixtures themselves. */
function fixtureKeys(room: RoomSpec): { whole: string[]; labels: string[] } {
  const whole: string[] = [];
  const labels: string[] = [];
  room.fixtures.forEach((f, i) => {
    switch (f.kind) {
      case "placard":
        whole.push("placard");
        break;
      case "terminal":
        whole.push(`terminal:${i}`);
        break;
      case "poster":
        whole.push(`poster:${i}`);
        break;
      case "door":
        labels.push(`door:${i}`);
        break;
      case "portal":
        labels.push(`portal:${i}`);
        break;
      case "hatch":
        labels.push(`hatch:${i}`);
        break;
      case "machine":
        labels.push(`tag:${i}`);
        break;
      case "lift":
        whole.push(`lift:${i}`);
        break;
      case "screen":
        whole.push(`screen:${i}`);
        break;
      case "exit":
        labels.push(`exit:${i}`);
        break;
    }
  });
  return { whole, labels };
}

describe("textRequests", () => {
  const requests = textRequests(bridge);

  it("asks for one request per text-bearing fixture, placard first", () => {
    expect(requests[0]?.kind).toBe("placard");
    expect(requests.filter((r) => r.kind === "screen")).toHaveLength(2);
    expect(requests.filter((r) => r.kind === "label")).toHaveLength(5);
    expect(requests.filter((r) => r.kind === "hatch")).toHaveLength(1);
    expect(requests.filter((r) => r.kind === "poster")).toHaveLength(1);
  });

  it("gives every request its own key", () => {
    expect(new Set(requests.map((r) => r.key)).size).toBe(requests.length);
  });

  it("puts the heading on a terminal screen's first line", () => {
    const screen = requests.find((r) => r.kind === "screen");
    expect(screen?.lines[0]).toBe("Scope");
  });

  it("gives labels a row and whole layers none", () => {
    for (const r of requests) {
      if (r.kind === "label" || r.kind === "hatch") {
        expect(r.row).not.toBeNull();
      } else {
        expect(r.row).toBeNull();
      }
    }
  });
});

describe("textRequests for the station's fixtures (M3 C7, C24, C28)", () => {
  const slot = { x: 1, y: 0, side: "n" } as const;
  const stop = (label: string, key = false, here = false): LiftStop => ({
    label,
    to: { kind: "airlock" },
    key,
    here,
  });
  const lift = (stops: LiftStop[], note: string | null): Fixture => ({
    kind: "lift",
    slot,
    stops,
    note,
    seed: 1,
  });
  const only = (...fixtures: Fixture[]): RoomSpec => ({ ...bridge, fixtures });

  it("lists ten stops, the overflow line and the note on a lift's panel", () => {
    // Mutation caught: the note dropped, the mark written after `> ` (the
    // renderer only draws the key for a line that starts with it), the
    // mark put on every line, the overflow line missing.
    const stops = Array.from({ length: 14 }, (_, i) =>
      stop(`S${String(i)}`, i === 0 || i === 3 || i === 12, i === 0),
    );
    const requests = textRequests(only(lift(stops, LIFT_WORDS.deckError)));
    expect(requests).toEqual([
      {
        key: "lift:0",
        kind: "panel",
        row: null,
        lines: [
          `${KEY_MARK}> S0`,
          "S1",
          "S2",
          `${KEY_MARK}S3`,
          "S4",
          "S5",
          "S6",
          "S7",
          "S8",
          "S9",
          "+4 MORE",
          "?DECK LIST ERROR",
        ],
      },
    ]);
  });

  it("writes no overflow line and no note when there is none", () => {
    // Mutation caught: the overflow line written when nothing overflows
    // (`+0 MORE`), a null note written as a line.
    const requests = textRequests(
      only(lift([stop("A"), stop("B", false, true), stop("C")], null)),
    );
    expect(requests.map((r) => r.lines)).toEqual([["A", "> B", "C"]]);
  });

  it("writes no overflow line for a lift of exactly LIFT_LINES stops", () => {
    // Mutation caught: the overflow test written `>=`, which gives a lift
    // of ten stops a `+0 MORE` line.
    const labels = Array.from(
      { length: LIFT_LINES },
      (_, i) => `S${String(i)}`,
    );
    const requests = textRequests(
      only(
        lift(
          labels.map((l) => stop(l)),
          null,
        ),
      ),
    );
    expect(requests.map((r) => r.lines)).toEqual([labels]);
  });

  it("marks only a screen's key lines, and labels an exit under its own key", () => {
    // Mutation caught: the key on every line of a screen, or on none; the
    // screen drawn as a terminal's `screen` kind (9 rows at the terminal's
    // aspect); the exit's label keyed `door:<i>` or drawn as a whole layer.
    const requests = textRequests(
      only(
        {
          kind: "screen",
          slot,
          lines: ["ALPHA", "3 ENGRAMS"],
          keys: [0],
          seed: 2,
        },
        {
          kind: "exit",
          slot: { x: 2, y: 0, side: "n" },
          label: "DECK 7 NOTES",
          to: { kind: "airlock" },
          seed: 3,
        },
      ),
    );
    expect(requests).toEqual([
      {
        key: "screen:0",
        kind: "station",
        row: null,
        lines: [`${KEY_MARK}ALPHA`, "3 ENGRAMS"],
      },
      { key: "exit:1", kind: "label", row: 0, lines: ["DECK 7 NOTES"] },
    ]);
  });
});

describe("LAYER", () => {
  it("keeps every procedural layer below TEXT_BASE and distinct (2.7 C12)", () => {
    // Mutation caught: a procedural layer's index moved to or past
    // TEXT_BASE, or two procedural layers sharing an index.
    expect(TEXT_BASE).toBe(11);
    const indices = Object.values(LAYER).sort((a, b) => a - b);
    expect(indices).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
  });
});

describe("layerPlan", () => {
  for (const [name, room] of [
    ["bridge", bridge],
    ["hub", hub],
    ["lifts hall", liftsHallRoom()],
  ] as const) {
    describe(name, () => {
      const plan = layerPlan(room);
      const { whole, labels } = fixtureKeys(room);

      it("counts the base layers, one per whole layer and one per six labels", () => {
        expect(plan.count).toBe(
          TEXT_BASE + whole.length + Math.ceil(labels.length / LABEL_ROWS),
        );
      });

      it("resolves every fixture key and throws on an unknown one", () => {
        for (const key of [...whole, ...labels]) {
          expect(() => plan.lookup(key)).not.toThrow();
        }
        expect(() => plan.lookup("door:9999")).toThrow();
        expect(() => plan.lookup("nothing")).toThrow();
      });

      it("puts every key in a text layer the array has", () => {
        for (const key of [...whole, ...labels]) {
          const slot = plan.lookup(key);
          expect(slot.layer).toBeGreaterThanOrEqual(TEXT_BASE);
          expect(slot.layer).toBeLessThan(plan.count);
        }
      });

      it("never puts two labels in the same layer row", () => {
        const rows = labels.map((key) => {
          const request = plan.text.find((r) => r.key === key);
          return `${plan.lookup(key).layer}:${request?.row ?? "none"}`;
        });
        expect(new Set(rows).size).toBe(labels.length);
      });

      it("never shares a whole layer with anything else", () => {
        const layers = [...whole, ...labels].map((k) => plan.lookup(k).layer);
        for (const key of whole) {
          const layer = plan.lookup(key).layer;
          expect(layers.filter((l) => l === layer)).toHaveLength(1);
        }
      });

      it("keeps v0 below v1 inside the layer", () => {
        for (const key of [...whole, ...labels]) {
          const { v0, v1 } = plan.lookup(key);
          expect(v0).toBeGreaterThanOrEqual(0);
          expect(v1).toBeLessThanOrEqual(1);
          expect(v0).toBeLessThan(v1);
        }
      });

      it("gives whole layers the whole height and labels their row less one texel", () => {
        for (const key of whole) {
          expect(plan.lookup(key)).toMatchObject({ v0: 0, v1: 1 });
        }
        for (const key of labels) {
          const { v0, v1 } = plan.lookup(key);
          expect(v1 - v0).toBeLessThanOrEqual(1 / LABEL_ROWS);
          expect(v1 - v0).toBeCloseTo((ROW_HEIGHT - 1) / LAYER_SIZE, 9);
        }
      });

      it("lists a request for every key, in the plan's text", () => {
        expect(plan.text.map((r) => r.key).sort()).toEqual(
          [...whole, ...labels].sort(),
        );
      });
    });
  }
});
