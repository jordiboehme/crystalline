/**
 * The probe's update field is read as a plain version or not at all (spec
 * S30): a missing field (an older server), `null` and any other shape all read
 * as no update.
 */

import { describe, expect, it } from "vitest";

import {
  releaseNotesUrl,
  updateAvailableOf,
  updateCheckOf,
} from "./updateNote";

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

  it("maps_update_check", () => {
    const on = {
      on: true,
      off_reason: null,
      install: "deb",
      last_checked: "2026-11-24T09:00:00Z",
      last_check: "up_to_date",
    };
    expect(updateCheckOf(on)).toEqual({
      on: true,
      offReason: null,
      deb: true,
      lastChecked: "2026-11-24T09:00:00Z",
      lastCheck: "up_to_date",
    });
    expect(
      updateCheckOf({
        ...on,
        on: false,
        off_reason: "docker",
        install: "docker",
      }),
    ).toEqual({
      on: false,
      offReason: "docker",
      deb: false,
      lastChecked: null,
      lastCheck: null,
    });
    // A check that has not run yet, and a time that does not parse, which
    // takes what the check found with it.
    for (const last_checked of [null, "yesterday"]) {
      expect(updateCheckOf({ ...on, last_checked })).toMatchObject({
        on: true,
        lastChecked: null,
        lastCheck: null,
      });
    }
    // A result word this page does not know drops only the result.
    expect(updateCheckOf({ ...on, last_check: "<b>new</b>" })).toMatchObject({
      lastChecked: "2026-11-24T09:00:00Z",
      lastCheck: null,
    });
    // Anything else reads as no state at all.
    for (const bad of [
      undefined,
      null,
      "on",
      {},
      { ...on, on: "yes" },
      { ...on, off_reason: "docker" },
      { ...on, on: false },
      { ...on, on: false, off_reason: "because" },
    ]) {
      expect(updateCheckOf(bad)).toBeNull();
    }
  });
});
