/**
 * The MANIFEST's addresses: the old read page, which lands on the domain
 * page now, and the editor, which an admin reaches and leaves from there.
 *
 * The editor half rides the same If-Match discipline the engram editor uses,
 * over `saveManifest` rather than `saveEngram`.
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

describe("the MANIFEST route", () => {
  it("lands the old manifest address on the domain page", async () => {
    serveAs("editor", {
      "/domains/eng/engrams": () => ({
        mode: "text",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
      "/vocabulary": () => ({ domain: "eng", tags: [] }),
      "/domains/eng/members": () => ({
        owner: null,
        visibility: "shared",
        members: [],
      }),
    });
    renderApp("/d/eng/manifest");
    expect(
      await screen.findByRole("heading", { name: "eng", level: 1 }),
    ).toBeInTheDocument();
    expect(
      await screen.findByRole("heading", { name: "Manifest", level: 2 }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: "MANIFEST", level: 1 }),
    ).toBeNull();
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
    expect(
      await screen.findByText(/this memory could not be recalled/i),
    ).toBeInTheDocument();
  });
});
