/**
 * Reading write provenance off a detail payload.
 *
 * `generated` is a mapping rather than a scalar, so the actor is a field inside
 * it. An engram written before that key existed carries none, and a reader that
 * turned the absence into a name would have the panel attribute a capture to
 * somebody who never made it.
 */

import { describe, expect, it } from "vitest";

import { readEngramDetail } from "./engram";

/** The payload for one engram, with whatever frontmatter a case needs. */
function detail(frontmatter: Record<string, unknown>) {
  return readEngramDetail(
    { domain: "eng", permalink: "alpha", frontmatter },
    "eng",
    "alpha",
  );
}

describe("the writer a detail payload names", () => {
  it("is the actor inside the generated mapping", () => {
    expect(
      detail({ generated: { by: "human:jordi" } }).frontmatter.generatedBy,
    ).toBe("human:jordi");
  });

  it("is nobody when the engram carries no generated block", () => {
    expect(detail({ title: "Alpha" }).frontmatter.generatedBy).toBeNull();
  });

  it("is nobody when the block names no actor", () => {
    expect(
      detail({ generated: { at: "2026-08-17T10:00:00+02:00" } }).frontmatter
        .generatedBy,
    ).toBeNull();
  });
});

describe("the neighbours advisory a write receipt carries", () => {
  it("reads the neighbours advisory and defaults to none", () => {
    const withSimilar = readEngramDetail(
      {
        domain: "eng",
        permalink: "retry-backoff-lesson",
        content: "",
        similar: [
          {
            domain: "eng",
            permalink: "retry-queue-gotcha",
            title: "Retry queue gotcha",
            status: "stable",
            type: "engram",
          },
          { permalink: "missing-domain" },
        ],
        guidance: "read the one that fits",
      },
      "eng",
      "retry-backoff-lesson",
    );
    expect(withSimilar.similar).toEqual([
      {
        domain: "eng",
        permalink: "retry-queue-gotcha",
        title: "Retry queue gotcha",
        status: "stable",
        type: "engram",
      },
    ]);
    expect(withSimilar.guidance).toBe("read the one that fits");

    const quiet = readEngramDetail(
      { domain: "eng", permalink: "alpha", content: "" },
      "eng",
      "alpha",
    );
    expect(quiet.similar).toEqual([]);
    expect(quiet.guidance).toBeNull();
  });
});
