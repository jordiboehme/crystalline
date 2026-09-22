/**
 * One address, two builders.
 *
 * The server hands an agent a browser URL for an engram and this app builds the
 * route a browser opens; a person pastes the first into the second. They have
 * to be the same bytes, or a handed URL lands on a page that says the engram is
 * not there - so both are held to the shared corpus at
 * `tests/fixtures/web-url/cases.json` rather than to each other's good
 * intentions.
 *
 * The third leg is the router: a path is only worth building if the app can
 * read the domain and the permalink back out of it, umlauts, spaces, brackets
 * and inner slashes included. That goes through `matchRoutes`, which is the
 * entry point that decodes a path segment by segment before matching, rather
 * than through `matchPath`, which decodes nothing.
 */

import { matchRoutes } from "react-router";
import { describe, expect, it } from "vitest";

import casesJson from "../../tests/fixtures/web-url/cases.json?raw";

import { headingSlug, sectionUrl } from "./anchors";
import { engramRoute } from "./paths";

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
}

const corpus = JSON.parse(casesJson) as Corpus;

describe("the engram route", () => {
  it("builds the route the server built", () => {
    for (const url of corpus.urls) {
      expect(engramRoute(url.domain, url.permalink)).toBe(
        new URL(url.web_url).pathname,
      );
    }
  });

  it("the router hands the domain and permalink back", () => {
    for (const url of corpus.urls) {
      const pathname = new URL(url.web_url).pathname;
      const matched = matchRoutes([{ path: "/d/:domain/e/*" }], pathname);
      expect(matched).not.toBeNull();
      expect(matched?.[0]?.params.domain).toBe(url.domain);
      expect(matched?.[0]?.params["*"]).toBe(url.permalink);
    }
  });
});

describe("the section url a reader copies", () => {
  it("the copied section url is the server's url plus the fragment", () => {
    for (const url of corpus.urls) {
      expect(sectionUrl(url.web_url, headingSlug(url.heading))).toBe(
        url.section_url,
      );
    }
  });
});
