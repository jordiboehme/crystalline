/**
 * The guided retire flow: three statuses, a successor picker gated to
 * "superseded", an optional valid_to bound, and the hard delete folded
 * behind it with an inbound-link warning fed by the graph neighborhood
 * rather than the detail payload's capped inbound sample.
 *
 * Mounted through `renderApp` on the engram page, the way `EngramEditor.test.tsx`
 * mounts the editor: the dialog needs the page's own cache wiring to prove the
 * retire status change lands optimistically, before any round trip answers.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import { singlePage } from "../api/engrams";
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

/** The detail payload, in the engine's own shape - mirrors EngramEditor.test.tsx. */
function detailResponse(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    permalink: "alpha",
    title: "Alpha",
    url: "crystalline://eng/alpha",
    path: "alpha.md",
    content:
      "---\ntitle: Alpha\npermalink: alpha\nstatus: stable\ntype: engram\n---\n\n# Alpha\n\nA rule.\n",
    checksum: "3f8a1c05e2",
    frontmatter: { engram_type: "engram", status: "stable", tags: [] },
    observations: [],
    relations: [],
    links: [],
    inbound: { count: 1, refs: [] },
    ...overrides,
  };
}

/** The neighborhood: one inbound neighbor, Citer, pointing at Alpha. */
function graphResponse() {
  return {
    nodes: [
      {
        id: 1,
        domain: "eng",
        permalink: "alpha",
        title: "Alpha",
        status: "stable",
        type: "engram",
      },
      {
        id: 2,
        domain: "eng",
        permalink: "citer",
        title: "Citer",
        status: "stable",
        type: "engram",
      },
    ],
    edges: [{ from: 2, to: 1, rel_type: "relates_to" }],
    truncated: false,
    hidden: 0,
  };
}

function serve(
  routes: Record<string, (path: string, init?: RequestInit) => unknown> = {},
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
      "/domains/eng/engrams/alpha": () => detailResponse(),
      "/graph": () => graphResponse(),
      ...routes,
    }),
  );
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the retire dialog", () => {
  it("retires with successor and valid_to, optimistically", async () => {
    const retired = vi.fn(() => ({
      domain: "eng",
      permalink: "alpha",
      status: "superseded",
      successor: "beta",
    }));
    serve({
      "/domains/eng/retire": (_path, init) =>
        init?.method === "POST" ? retired() : null,
      "/search": () =>
        singlePage([
          {
            domain: "eng",
            permalink: "beta",
            title: "Beta",
            type: null,
            status: null,
            tags: [],
            kind: null,
            line: null,
            snippet: null,
          },
        ]),
    });
    renderApp("/d/eng/e/alpha");
    await userEvent.click(
      await screen.findByRole("button", { name: "Retire" }),
    );
    const dialog = await screen.findByRole("dialog", { name: /retire/i });
    await userEvent.click(
      within(dialog).getByRole("radio", { name: "superseded" }),
    );
    await userEvent.type(within(dialog).getByLabelText("Successor"), "Beta");
    await userEvent.click(
      await within(dialog).findByRole("option", { name: /Beta/ }),
    );
    await userEvent.type(
      within(dialog).getByLabelText("Valid to"),
      "2026-08-09",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Retire engram" }),
    );
    await waitFor(() => {
      expect(retired).toHaveBeenCalled();
    });
    const call = apiMock.mock.calls.find(
      ([sent]) => sent === "/domains/eng/retire",
    );
    const body = call?.[1]?.body;
    if (typeof body !== "string") {
      throw new Error("no retire body");
    }
    expect(JSON.parse(body) as unknown).toEqual({
      permalink: "alpha",
      status: "superseded",
      successor: "beta",
      valid_to: "2026-08-09",
    });
    // Optimistic: the lifecycle banner is up without a refetch round trip.
    const banner = await screen.findByRole("note");
    expect(within(banner).getByText(/superseded/i)).toBeInTheDocument();
  });

  it("delete hides behind retire and warns with who points here", async () => {
    const deleted = vi.fn(() => undefined);
    serve({
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "DELETE" ? deleted() : detailResponse(),
    });
    renderApp("/d/eng/e/alpha");
    await userEvent.click(
      await screen.findByRole("button", { name: "Retire" }),
    );
    const dialog = await screen.findByRole("dialog", { name: /retire/i });
    await userEvent.click(
      within(dialog).getByRole("button", {
        name: "Delete permanently instead",
      }),
    );
    // The warning names the inbound neighbor the graph reported.
    expect(await within(dialog).findByText(/Citer/)).toBeInTheDocument();
    expect(
      within(dialog).getByText(/1 reference into this engram would break/i),
    ).toBeInTheDocument();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Delete permanently" }),
    );
    await waitFor(() => {
      expect(deleted).toHaveBeenCalled();
    });
    const call = apiMock.mock.calls.find(
      ([, init]) => init?.method === "DELETE",
    );
    expect(call?.[1]?.headers).toEqual({ "If-Match": '"3f8a1c05e2"' });
  });
});
