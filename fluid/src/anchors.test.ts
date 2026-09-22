/**
 * The slug rule, held to the shared corpus.
 *
 * A fragment is a promise between two writers: an agent derives it from the
 * heading text it already has, and this app assigns the id a browser lands on.
 * If either side changed its mind about a space, an ampersand or an umlaut the
 * two would name different sections of the same page, so both read the same
 * cases out of the same file rather than each keeping its own idea of them.
 */

import { describe, expect, it } from "vitest";

// The shared corpus, as text, out of the very file the service's own test
// reads: `tests/fixtures/web-url/cases.json` at the repository root. `?raw`
// rather than `node:fs` because this program is browser scoped and carries no
// Node types; vite.config.ts allows the one folder it lives in for the test
// run.
import casesJson from "../../tests/fixtures/web-url/cases.json?raw";

import { headingSlug, sectionUrl, slugAssigner } from "./anchors";

interface Corpus {
  urls: {
    base: string;
    domain: string;
    permalink: string;
    web_url: string;
    heading: string;
    fragment: string;
    section_url: string;
  }[];
  slugs: { headings: string[]; fragments: string[] }[];
}

const corpus = JSON.parse(casesJson) as Corpus;

describe("the heading slug", () => {
  it("slugs the fixture's headings the way the spec says", () => {
    for (const url of corpus.urls) {
      expect(headingSlug(url.heading)).toBe(url.fragment);
    }
  });

  it("keeps non-ascii letters and marks rather than transliterating", () => {
    expect(headingSlug("Über uns")).toBe("über-uns");
    // A combining mark is part of the letter it sits on and stays with it.
    expect(headingSlug("Café notes")).toBe("café-notes");
    expect(headingSlug("Auth & Tokens")).toBe("auth-tokens");
    expect(headingSlug("Step 2: verify")).toBe("step-2-verify");
  });

  it("an empty heading becomes section", () => {
    expect(headingSlug("")).toBe("section");
    expect(headingSlug("   ")).toBe("section");
    // Nothing survives the filter here either, so there is no slug to make.
    expect(headingSlug("!!! ???")).toBe("section");
  });
});

describe("the assigner one document runs on", () => {
  it("numbers repeats in document order and never reuses a taken slug", () => {
    for (const run of corpus.slugs) {
      const assign = slugAssigner();
      expect(run.headings.map(assign)).toEqual(run.fragments);
    }
  });

  it("starts over for the next document", () => {
    const first = slugAssigner();
    const second = slugAssigner();
    expect(first("API")).toBe("api");
    expect(second("API")).toBe("api");
  });
});

describe("the section url", () => {
  it("is the page url and the fragment, written as they are", () => {
    for (const url of corpus.urls) {
      expect(sectionUrl(url.web_url, url.fragment)).toBe(url.section_url);
    }
  });
});
