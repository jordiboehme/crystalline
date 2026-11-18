/**
 * The probe's update field is read as a plain version or not at all (spec
 * S30): a missing field (an older server), `null` and any other shape all read
 * as no update.
 */

import { describe, expect, it } from "vitest";

import { releaseNotesUrl, updateAvailableOf } from "./updateNote";

describe("the update field of the probe", () => {
  it("maps_update_available", () => {
    expect(updateAvailableOf("0.35.0")).toBe("0.35.0");
    expect(updateAvailableOf(undefined)).toBeNull();
    expect(updateAvailableOf(null)).toBeNull();
    expect(updateAvailableOf("0.35.0<img src=x>")).toBeNull();
    expect(updateAvailableOf("v0.35.0")).toBeNull();
    expect(updateAvailableOf("0.35.0-rc.1")).toBeNull();
    expect(releaseNotesUrl("0.35.0")).toBe(
      "https://github.com/jordiboehme/crystalline/releases/tag/v0.35.0",
    );
  });
});
