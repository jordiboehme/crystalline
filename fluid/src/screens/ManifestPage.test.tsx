/**
 * The MANIFEST: pinned first in the sidebar tree, read on its own page, and
 * edited there by an admin only.
 *
 * The pin is presentation over an address every domain already answers at -
 * `GET /domains/{d}/manifest` - so what this file proves is the route wiring
 * and the gate, not a new fetch. The editor half rides the same If-Match
 * discipline the engram editor uses, over `saveManifest` rather than
 * `saveEngram`.
 */

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import type { Answer } from "../test/harness";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

/** The manifest payload, in the engine's own shape. */
function manifestResponse(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    markdown: "# eng\n\nRoute here for eng questions.\n",
    checksum: "m1",
    ...overrides,
  };
}

/** An empty tree, so the sidebar under a manifest screen mounts with nothing
 *  else to say. */
function emptyTree() {
  return { domain: "eng", path: "", folders: [], engrams: [] };
}

/** A minimal engram, for the one test that opens through the engram page. */
function alphaResponse() {
  return {
    domain: "eng",
    permalink: "alpha",
    title: "Alpha",
    type: "engram",
    status: "stable",
    url: "crystalline://eng/alpha",
    content: "Alpha's body.",
    checksum: "abc123",
    frontmatter: { engram_type: "engram", status: "stable", tags: [] },
    observations: [],
    relations: [],
    links: [],
  };
}

/** Everything the shell needs to open a domain, an ordinary session. */
function serve() {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
      "/domains/eng/manifest": () => manifestResponse(),
      "/domains/eng/tree": () => emptyTree(),
      "/domains/eng/engrams/alpha": () => alphaResponse(),
      "/graph": () => ({ nodes: [], edges: [], truncated: false }),
    }),
  );
}

/** The app, signed in at the given role, with the manifest and an empty
 *  domain tree served underneath it. */
function serveAs(
  role: "admin" | "editor",
  routes: Record<string, Answer> = {},
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role }) }),
      "/domains": domainsResponse,
      "/domains/eng/manifest": () => manifestResponse(),
      "/domains/eng/tree": () => emptyTree(),
      "/validate": () => ({ findings: [], errors: 0 }),
      ...routes,
    }),
  );
}

/** The parsed body of the `index`th PUT the mock has seen, in call order. */
function putBody(index: number): unknown {
  const calls = apiMock.mock.calls.filter(([, init]) => init?.method === "PUT");
  const body = calls[index]?.[1]?.body;
  if (typeof body !== "string") {
    throw new Error(`no PUT body at index ${index}`);
  }
  return JSON.parse(body) as unknown;
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the MANIFEST", () => {
  it("the sidebar pins MANIFEST first, apart from the engrams", async () => {
    serve();
    renderApp("/d/eng/e/alpha");
    const nav = await screen.findByRole("link", { name: "MANIFEST" });
    expect(nav).toHaveAttribute("href", "/d/eng/manifest");
    // Not the current screen here - an engram page is open, not the manifest.
    expect(nav).not.toHaveAttribute("aria-current");
  });

  it("the manifest page renders the markdown and offers Edit to an admin only", async () => {
    serveAs("admin");
    renderApp("/d/eng/manifest");
    expect(
      await screen.findByRole("heading", { name: "MANIFEST", level: 1 }),
    ).toBeInTheDocument();
    // The trail above says "eng > MANIFEST", so the title does not say the
    // domain a second time - the line under it does, once.
    expect(
      screen.getByText("The eng domain, in its own words."),
    ).toBeInTheDocument();
    expect(
      await screen.findByText(/Route here for eng questions/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: "Edit MANIFEST" }),
    ).toBeInTheDocument();
    // The pinned row marks itself current the same way an engram row does.
    expect(screen.getByRole("link", { name: "MANIFEST" })).toHaveAttribute(
      "aria-current",
      "page",
    );
  });

  it("no Edit below admin", async () => {
    serveAs("editor");
    renderApp("/d/eng/manifest");
    await screen.findByRole("heading", { name: /manifest/i });
    expect(
      screen.queryByRole("link", { name: "Edit MANIFEST" }),
    ).not.toBeInTheDocument();
  });

  it("the editor saves the manifest with its If-Match token", async () => {
    const put = vi.fn(() => ({
      domain: "eng",
      markdown: "# eng",
      checksum: "m2",
    }));
    serveAs("admin", {
      "/domains/eng/manifest": (_path, init) =>
        init?.method === "PUT"
          ? put()
          : { domain: "eng", markdown: "# eng", checksum: "m1" },
    });
    renderApp("/d/eng/manifest/edit");
    await screen.findByLabelText("MANIFEST source");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(put).toHaveBeenCalled();
    });
    const call = apiMock.mock.calls.find(([, init]) => init?.method === "PUT");
    expect(call?.[1]?.headers).toEqual({ "If-Match": '"m1"' });
    // What went on the wire is the buffer's own text: nothing was typed, so
    // it is exactly what the GET answered.
    expect(putBody(0)).toEqual({ markdown: "# eng" });
  });

  it("is not offered to a non-admin, same as the not-found screen", async () => {
    serveAs("editor");
    renderApp("/d/eng/manifest/edit");
    await waitFor(() => {
      expect(
        screen.queryByLabelText("MANIFEST source"),
      ).not.toBeInTheDocument();
    });
    expect(await screen.findByText(/this memory could not be recalled/i)).toBeInTheDocument();
  });
});
