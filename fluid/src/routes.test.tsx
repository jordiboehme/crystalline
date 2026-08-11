/**
 * The one route that can go wrong quietly: an engram's.
 *
 * A permalink is a path of its own, so the link builder, the URL and the splat
 * param have to agree about that all the way through. If any of the three
 * treats it as a single segment instead, `notes/deep/gamma` becomes
 * `notes%2Fdeep%2Fgamma` and the app navigates to a different, missing engram -
 * a 404 nobody can explain from the link they clicked. So this walks the whole
 * round trip through the real router, and it starts from the links a reader
 * actually clicks: the rows of a domain's engram list.
 *
 * The round trip ends at the request the engram screen makes, because that is
 * the last place the permalink can be mangled: the API path has to carry the
 * same slashes the link did, encoded a segment at a time.
 */

import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api, engramPath } from "./api/client";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "./test/harness";

vi.mock("./api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

/** A domain whose one engram sits at `permalink`, and that engram itself. */
function serveDomain(domain: string, permalink: string) {
  const encoded = encodeURIComponent(domain);
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
      [`/domains/${encoded}/manifest`]: () => ({ domain, markdown: "" }),
      [`/domains/${encoded}/tree`]: () => ({
        domain,
        path: "/",
        folders: [],
        engrams: [
          {
            permalink,
            title: "Gamma",
            type: "engram",
            path: `${permalink}.md`,
          },
        ],
      }),
      // The screen's list is the paged listing, scoped to the folder it is
      // browsing; the tree above it is the folder navigation. The row this
      // test clicks is a row of the listing.
      [`/domains/${encoded}/engrams`]: () => ({
        mode: "text",
        total: 1,
        page: 1,
        limit: 50,
        count: 1,
        hits: [
          {
            domain,
            permalink,
            title: "Gamma",
            engram_type: "engram",
            kind: "engram",
            status: "stable",
            tags: [],
          },
        ],
      }),
      "/vocabulary": () => ({ domain, tags: [] }),
      [engramPath(domain, permalink)]: () => ({
        domain,
        permalink,
        title: "Gamma",
        type: "engram",
        status: "stable",
        url: `crystalline://${domain}/${permalink}`,
        content: "The third engram.",
        checksum: "abc123",
        frontmatter: { engram_type: "engram", status: "stable", tags: [] },
        observations: [],
        relations: [],
        links: [],
      }),
      "/graph": () => ({ nodes: [], edges: [], truncated: false }),
    }),
  );
}

/** Every path the app asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => call[0]);
}

/**
 * Open the domain and click the row the list drew for its one engram.
 *
 * Scoped to the screen: inside a domain the frame's sidebar draws the same
 * engram as a tree entry, so an unscoped query would be asking about two
 * links at once.
 */
async function followRowIn(domain: string, permalink: string) {
  serveDomain(domain, permalink);
  renderApp(`/d/${encodeURIComponent(domain)}`);
  const screenBody = await screen.findByRole("main");
  const row = await within(screenBody).findByRole("link", { name: /Gamma/ });
  await userEvent.click(row);
  return row;
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the engram route", () => {
  it("carries a multi-segment permalink through the link and the splat", async () => {
    const row = await followRowIn("eng", "notes/deep/gamma");

    // The slashes inside the permalink stay slashes in the URL, which is what
    // makes the splat match rather than a single escaped segment.
    expect(row).toHaveAttribute("href", "/d/eng/e/notes/deep/gamma");
    expect(await screen.findByRole("heading", { name: "Gamma" })).toBeVisible();
    // And the same slashes reach the API, which is the only proof the splat
    // arrived whole rather than merely matching a route.
    expect(requested()).toContain("/domains/eng/engrams/notes/deep/gamma");
  });

  it("round-trips a segment that needed encoding, decoded", async () => {
    const row = await followRowIn("eng", "notes/deep dive/gamma");

    expect(row).toHaveAttribute("href", "/d/eng/e/notes/deep%20dive/gamma");
    // Encoded on the way out, decoded on the way in, and encoded again a
    // segment at a time on the way to the API: the screen sees the permalink
    // as it is written on disk, not as it travelled.
    expect(await screen.findByRole("heading", { name: "Gamma" })).toBeVisible();
    expect(requested()).toContain(
      "/domains/eng/engrams/notes/deep%20dive/gamma",
    );
  });

  it("carries a domain that needed encoding too", async () => {
    await followRowIn("team eng", "notes/alpha");

    expect(await screen.findByRole("heading", { name: "Gamma" })).toBeVisible();
    expect(requested()).toContain("/domains/team%20eng/engrams/notes/alpha");
  });
});
