/**
 * Every top-level screen names its page: the tab, the history and a saved PDF
 * read `<page> - Fluid`, and a page with no title yet reads `Fluid`.
 */

import { screen, waitFor } from "@testing-library/react";
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

function serve(routes: Record<string, () => unknown> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
      "/vocabulary": () => ({ tags: [], categories: [], relation_types: [] }),
      "/search": () => ({
        mode: "text",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
      "/graph": () => ({ nodes: [], edges: [], truncated: false }),
      ...routes,
    }),
  );
}

function engram(title: string) {
  return {
    domain: "eng",
    permalink: "alpha",
    title,
    type: "engram",
    status: "stable",
    url: "crystalline://eng/alpha",
    content: "Text.",
    checksum: "abc",
    frontmatter: { engram_type: "engram", status: "stable", tags: [] },
    observations: [],
    relations: [],
    links: [],
  };
}

async function titled(expected: string) {
  await waitFor(() => {
    expect(document.title).toBe(expected);
  });
}

beforeEach(() => {
  apiMock.mockReset();
  document.title = "Fluid";
});

describe("page titles", () => {
  it("names Home", async () => {
    serve();
    renderApp("/");
    await titled("Home - Fluid");
  });

  it("names Search and the Graph", async () => {
    serve();
    const first = renderApp("/search?q=gamma");
    await titled("Search - Fluid");
    first.unmount();
    renderApp("/graph");
    await titled("Graph - Fluid");
  });

  it("names an engram by its title, keeping slashes and quotes as text", async () => {
    const title = `A/B "final" plan`;
    serve({ [engramPath("eng", "alpha")]: () => engram(title) });
    renderApp("/d/eng/e/alpha");
    await titled(`${title} - Fluid`);
  });

  it("says Fluid for an engram that is not found", async () => {
    serve();
    renderApp("/d/eng/e/missing");
    await screen.findByRole("main");
    expect(document.title).toBe("Fluid");
  });

  it("says Fluid for an address nothing answers", async () => {
    serve();
    renderApp("/no/such/page");
    await screen.findByRole("heading", { level: 1 });
    expect(document.title).toBe("Fluid");
  });

  it("names the domain home by the domain", async () => {
    serve({
      "/domains/eng/tree": () => ({
        domain: "eng",
        path: "/",
        folders: [],
        engrams: [],
      }),
      "/domains/eng/manifest": () => ({ domain: "eng", markdown: "" }),
    });
    renderApp("/d/eng/engrams");
    await titled("eng - Fluid");
  });

  it("names nothing for an admin screen a non-admin is refused", async () => {
    serve();
    renderApp("/users");
    await screen.findByRole("main");
    expect(document.title).toBe("Fluid");
  });
});
