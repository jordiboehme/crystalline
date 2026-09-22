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

  it("carries the model the writer reported, beside the actor", () => {
    const frontmatter = detail({
      generated: { by: "claude-code/2.1.271", model: "claude-opus-5" },
    }).frontmatter;
    expect(frontmatter.generatedBy).toBe("claude-code/2.1.271");
    expect(frontmatter.generatedModel).toBe("claude-opus-5");
  });

  it("reports no model where the block names none", () => {
    expect(
      detail({ generated: { by: "human:jordi" } }).frontmatter.generatedModel,
    ).toBeNull();
  });
});

describe("the model a verification records", () => {
  it("is read beside the verifier", () => {
    const entries = detail({
      verified: [
        {
          by: "claude-code/2.1.271",
          model: "claude-opus-5",
          at: "2026-09-16T10:04:01+00:00",
        },
      ],
    }).frontmatter.verified;
    expect(entries[0]?.by).toBe("claude-code/2.1.271");
    expect(entries[0]?.model).toBe("claude-opus-5");
  });

  it("is nothing where the entry names none", () => {
    const entries = detail({
      verified: [{ by: "human:jordi", at: "2026-09-16T10:04:01+00:00" }],
    }).frontmatter.verified;
    expect(entries[0]?.model).toBeNull();
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

describe("the page address a detail payload carries", () => {
  it("reads web_url and tolerates its absence", () => {
    const served = readEngramDetail(
      {
        domain: "eng",
        permalink: "alpha",
        web_url: "https://kb.example.com/d/eng/e/alpha",
      },
      "eng",
      "alpha",
    );
    expect(served.webUrl).toBe("https://kb.example.com/d/eng/e/alpha");

    // A server that could not work out an address says so in `web_url_note`
    // and carries no `web_url` at all, so the page has no browser URL to
    // hand over and says nothing rather than inventing one.
    const unresolved = readEngramDetail(
      { domain: "eng", permalink: "alpha" },
      "eng",
      "alpha",
    );
    expect(unresolved.webUrl).toBeNull();
  });

  it("reads local_change and tolerates its absence", () => {
    expect(
      readEngramDetail({ local_change: "modified" }, "eng", "alpha")
        .localChange,
    ).toBe("modified");
    expect(
      readEngramDetail({ local_change: "added" }, "eng", "alpha").localChange,
    ).toBe("added");
    expect(readEngramDetail({}, "eng", "alpha").localChange).toBeNull();
    // Any other word is not a claim this side knows how to draw.
    expect(
      readEngramDetail({ local_change: "renamed" }, "eng", "alpha").localChange,
    ).toBeNull();
  });
});
