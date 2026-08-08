/**
 * The editor screen: what it loads, who it is offered to, and what a save is
 * allowed to do.
 *
 * The whole file is the document - frontmatter included - because that is what
 * the engine writes back, and the save carries the If-Match token of the
 * version it read. A save that renamed the engram through its frontmatter
 * answers at the new address, and the editor follows it there rather than
 * leaving somebody editing a page that now 404s.
 */

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
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

const CONTENT =
  "---\ntitle: Alpha\npermalink: alpha\nstatus: stable\ntype: engram\n---\n\n# Alpha\n\nA rule.\n";

/** The detail payload, in the engine's own shape. */
function detailResponse(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    permalink: "alpha",
    title: "Alpha",
    url: "crystalline://eng/alpha",
    path: "alpha.md",
    content: CONTENT,
    checksum: "3f8a1c05e2",
    frontmatter: { engram_type: "engram", status: "stable", tags: [] },
    observations: [],
    relations: [],
    links: [],
    ...overrides,
  };
}

function serveEditor(
  routes: Record<string, (path: string, init?: RequestInit) => unknown> = {},
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
      "/domains/eng/engrams/alpha": () => detailResponse(),
      "/validate": () => ({ findings: [], errors: 0 }),
      ...routes,
    }),
  );
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the engram editor", () => {
  it("loads the exact file text into the buffer", async () => {
    serveEditor();
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(editor.textContent).toContain("A rule.");
    });
    // Frontmatter is in the buffer too: the whole file is the document.
    expect(editor.textContent).toContain("permalink: alpha");
  });

  it("is not offered to a viewer", async () => {
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture({ role: "viewer" }) }),
        "/domains": domainsResponse,
      }),
    );
    renderApp("/d/eng/edit/alpha");
    await waitFor(() => {
      expect(screen.queryByLabelText("Engram source")).not.toBeInTheDocument();
    });
  });

  it("saves with the If-Match token and reports it", async () => {
    const put = vi.fn(() => detailResponse({ checksum: "next111" }));
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT" ? put() : detailResponse(),
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(put).toHaveBeenCalled();
    });
    const call = apiMock.mock.calls.find(([, init]) => init?.method === "PUT");
    expect(call?.[1]?.headers).toEqual({ "If-Match": '"3f8a1c05e2"' });
    expect(await screen.findByText("Saved")).toBeInTheDocument();
  });

  it("saves from inside the buffer, on the keyboard", async () => {
    const put = vi.fn(() => detailResponse({ checksum: "next111" }));
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT" ? put() : detailResponse(),
    });
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    editor.focus();
    // Mod is Control off the Mac, which is what the test environment says it
    // is running on.
    await userEvent.keyboard("{Control>}s{/Control}");
    await waitFor(() => {
      expect(put).toHaveBeenCalled();
    });
  });

  it("follows a rename to the engram's new address", async () => {
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT"
          ? detailResponse({ permalink: "sharper-alpha", checksum: "next222" })
          : detailResponse(),
      "/domains/eng/engrams/sharper-alpha": () =>
        detailResponse({ permalink: "sharper-alpha", checksum: "next222" }),
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    // The header echoes the address the engram now answers at.
    expect(await screen.findByText("sharper-alpha")).toBeInTheDocument();
  });
});
