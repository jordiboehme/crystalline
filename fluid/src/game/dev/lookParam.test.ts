import { describe, expect, it } from "vitest";

import { LOOK_ORDER } from "../render/looks";
import { lookLabel, lookParam } from "./lookParam";

describe("the dev pages' ?look=", () => {
  it("picks any look by its id, and the game's own when absent or unknown", () => {
    // Mutation caught: a default other than the game's own look, or an
    // unknown name let through as a look.
    const at = (search: string) => lookParam(new URLSearchParams(search));
    for (const id of LOOK_ORDER) expect(at(`?look=${id}`)).toBe(id);
    expect(at("")).toBe("aperture");
    expect(at("?look=neon")).toBe("aperture");
    expect(lookLabel("aperture")).toBe("APERTURE GRID");
  });
});
