import { describe, expect, it } from "vitest";

import { CANNED_BRIDGE } from "../world/canned";
import { generateRoom } from "../world/generate";
import { TEXT_BASE, layerCount, textRequests } from "./layers";

describe("textRequests", () => {
  const room = generateRoom(CANNED_BRIDGE);
  const requests = textRequests(room);

  it("asks for one text layer per text-bearing fixture, placard first", () => {
    expect(requests[0]?.kind).toBe("placard");
    expect(requests.filter((r) => r.kind === "screen")).toHaveLength(2);
    expect(requests.filter((r) => r.kind === "label")).toHaveLength(6);
    expect(requests.filter((r) => r.kind === "placard")).toHaveLength(2);
  });

  it("gives every request its own key", () => {
    expect(new Set(requests.map((r) => r.key)).size).toBe(requests.length);
  });

  it("counts the layers the texture array needs", () => {
    expect(layerCount(room)).toBe(TEXT_BASE + requests.length);
  });

  it("puts the heading on a terminal screen's first line", () => {
    const screen = requests.find((r) => r.kind === "screen");
    expect(screen?.lines[0]).toBe("Scope");
  });
});
