import { describe, expect, it } from "vitest";

import { CANNED_BRIDGE, CANNED_HUB } from "../world/canned";
import { generateRoom } from "../world/generate";
import type { RoomSpec } from "../world/types";
import {
  LABEL_ROWS,
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

describe("layerPlan", () => {
  for (const [name, room] of [
    ["bridge", bridge],
    ["hub", hub],
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
