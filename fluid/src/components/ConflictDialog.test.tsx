/**
 * Settling a conflict from the browser: both sides read side by side, one of
 * them taken, or a text somebody wrote out of the two.
 *
 * Mounted through the domain screen rather than in isolation, for the same
 * reason the share dialog is: the only way in is the sync card's conflict list,
 * which is admin-only, and a dialog tested on its own would pass while nothing
 * on the screen could open it.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { Answer } from "../test/harness";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";

// Above roughly load average 33 this file's slower tests exceed the 5000 ms
// default (a threshold effect measured 2026-08-14, plans history); the raise
// keeps a loaded machine from reading as a failure. Never raise the global
// default to hide this.
vi.setConfig({ testTimeout: 15000 });

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

/**
 * The sync status in the shape the real route sends, carrying one conflict as
 * itself: id and path, which is what makes it a thing the card can open rather
 * than a number it can only count.
 */
function syncResponse(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    mode: "github",
    repo: "acme/knowledge",
    branch: "main",
    base_commit: "9f3c1a2",
    behind: false,
    local_changes: 1,
    last_checked: "2026-08-21T08:00:00Z",
    probe_error: null,
    connection: { connected: true, user: "octo", token_store: "keychain" },
    open_proposals: [],
    declined_proposals: [],
    conflicts: [
      {
        id: "abc12345",
        path: "notes/a.md",
        kind: "EditEdit",
        base_commit: "9f3c1a2",
        upstream_commit: "b2c3d4e",
        detected_at: "2026-08-21T09:00:00Z",
      },
    ],
    ...overrides,
  };
}

/** One conflict with every side of it, as the detail route sends it. */
function detailResponse(overrides: Record<string, unknown> = {}) {
  return {
    id: "abc12345",
    path: "notes/a.md",
    kind: "EditEdit",
    base: "base text",
    local: "my local text",
    upstream: "the team's text",
    note: null,
    ...overrides,
  };
}

function serve(routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role: "admin" }) }),
      "/domains": domainsResponse,
      "/domains/eng/manifest": () => ({ domain: "eng", markdown: "# eng\n" }),
      "/domains/eng/tree": () => ({
        domain: "eng",
        path: "/",
        folders: [],
        engrams: [],
      }),
      "/domains/eng/engrams": () => ({
        mode: "text",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
      "/vocabulary": () => ({
        domain: "eng",
        tags: [],
        categories: [],
        relation_types: [],
      }),
      "/domains/eng/sync": () => syncResponse(),
      "/domains/eng/sync/conflicts/abc12345": () => detailResponse(),
      ...routes,
    }),
  );
}

/** Every path the app asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => call[0]);
}

/** How many times a route was asked for, by path prefix. */
function reads(prefix: string): number {
  return requested().filter((path) => path.startsWith(prefix)).length;
}

/**
 * Every read a settled conflict can invalidate, counted at once: the folders
 * the navigation walks, the listing the screen pages and the domain listing
 * every sidebar, card and switcher draws from.
 */
function contentReads(): [number, number, number] {
  return [
    reads("/domains/eng/tree"),
    reads("/domains/eng/engrams"),
    requested().filter((path) => path === "/domains").length,
  ];
}

/** The body of the request the app sent to `path` with `method`, parsed. */
function sentBody(path: string, method: string): unknown {
  const call = apiMock.mock.calls.find(
    ([sent, init]) => sent === path && init?.method === method,
  );
  if (!call) {
    throw new Error(`no ${method} to ${path}`);
  }
  const body = call[1]?.body;
  if (typeof body !== "string") {
    throw new Error(`the ${method} to ${path} carried no JSON body`);
  }
  return JSON.parse(body) as unknown;
}

/** A resolve route that records what it was asked to do. */
function resolveRoute(): { route: Answer; called: () => boolean } {
  const spy = vi.fn(() => ({ resolved: "notes/a.md", remaining: 0 }));
  return {
    route: (_path, init) => (init?.method === "POST" ? spy() : null),
    called: () => spy.mock.calls.length > 0,
  };
}

/** Open the dialog off the sync card's conflict list, once the card is up. */
async function openConflictDialog(): Promise<HTMLElement> {
  const card = await screen.findByRole("region", { name: "Team sync" });
  await userEvent.click(
    within(card).getByRole("button", { name: "notes/a.md" }),
  );
  return screen.findByRole("dialog", { name: /conflict/i });
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the conflict dialog", () => {
  it("shows both sides and takes theirs", async () => {
    const resolve = resolveRoute();
    serve({ "/domains/eng/sync/conflicts/abc12345/resolve": resolve.route });

    renderApp("/d/eng");
    const dialog = await openConflictDialog();

    // Both texts, as themselves: deciding between two versions is impossible
    // without reading them, and a diff nobody can see is a coin toss.
    expect(await within(dialog).findByText("my local text")).toBeVisible();
    expect(within(dialog).getByText("the team's text")).toBeVisible();

    await userEvent.click(
      within(dialog).getByRole("button", { name: "Take theirs" }),
    );

    await waitFor(() => {
      expect(resolve.called()).toBe(true);
    });
    expect(
      sentBody("/domains/eng/sync/conflicts/abc12345/resolve", "POST"),
    ).toEqual({ resolution: "theirs" });
    // Settled, so the dialog leaves rather than sitting on a conflict that is
    // no longer one.
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: /conflict/i })).toBeNull();
    });
  });

  it("keeps mine on the other button", async () => {
    const resolve = resolveRoute();
    serve({ "/domains/eng/sync/conflicts/abc12345/resolve": resolve.route });

    renderApp("/d/eng");
    const dialog = await openConflictDialog();

    await userEvent.click(
      await within(dialog).findByRole("button", { name: "Keep mine" }),
    );

    await waitFor(() => {
      expect(resolve.called()).toBe(true);
    });
    // The counterweight to the test above: the two buttons are two different
    // words on the wire, not one button with a label that reads both ways.
    expect(
      sentBody("/domains/eng/sync/conflicts/abc12345/resolve", "POST"),
    ).toEqual({ resolution: "mine" });
  });

  it("labels a deleted side and saves a hand merge", async () => {
    const resolve = resolveRoute();
    serve({
      "/domains/eng/sync/conflicts/abc12345": () =>
        detailResponse({ kind: "EditDelete", upstream: null }),
      "/domains/eng/sync/conflicts/abc12345/resolve": resolve.route,
    });

    renderApp("/d/eng");
    const dialog = await openConflictDialog();

    // A side that is not there is said in words: an empty pane reads as an
    // empty file, which is a different thing to decide about.
    expect(await within(dialog).findByText("(file deleted)")).toBeVisible();

    await userEvent.click(
      within(dialog).getByRole("button", { name: "Edit merged" }),
    );
    const editor = within(dialog).getByLabelText("Merged content");
    // Prefilled with this copy's own text, because a merge is almost always
    // somebody's own version with the team's changes worked into it.
    expect(editor).toHaveValue("my local text");
    await userEvent.clear(editor);
    await userEvent.type(editor, "reconciled text");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Save merged" }),
    );

    await waitFor(() => {
      expect(resolve.called()).toBe(true);
    });
    expect(
      sentBody("/domains/eng/sync/conflicts/abc12345/resolve", "POST"),
    ).toEqual({ resolution: "merged", content: "reconciled text" });
  });

  it("says a side is unreadable rather than deleted when a note explains it", async () => {
    serve({
      "/domains/eng/sync/conflicts/abc12345": () =>
        detailResponse({
          upstream: null,
          note: "the team's side is not valid UTF-8",
        }),
    });

    renderApp("/d/eng");
    const dialog = await openConflictDialog();

    // The two reasons a side arrives empty are not the same decision: a file
    // the team deleted can be taken as a deletion, while bytes this side
    // cannot show are still there and still theirs.
    expect(
      await within(dialog).findByText("(no readable content)"),
    ).toBeVisible();
    expect(within(dialog).queryByText("(file deleted)")).toBeNull();
    // And the server's own sentence, which is the only thing that says which
    // side could not be read and why.
    expect(within(dialog).getByText(/not valid UTF-8/)).toBeVisible();
  });

  it("reads the domain again once a conflict is settled", async () => {
    const resolve = resolveRoute();
    serve({ "/domains/eng/sync/conflicts/abc12345/resolve": resolve.route });

    renderApp("/d/eng");
    const dialog = await openConflictDialog();
    const statusReads = requested().filter(
      (path) => path === "/domains/eng/sync",
    ).length;
    const [trees, listings, listed] = contentReads();

    await userEvent.click(
      await within(dialog).findByRole("button", { name: "Take theirs" }),
    );

    await waitFor(() => {
      expect(resolve.called()).toBe(true);
    });
    // The status behind the card, because this conflict is no longer one of
    // its numbers.
    await waitFor(() => {
      expect(
        requested().filter((path) => path === "/domains/eng/sync").length,
      ).toBeGreaterThan(statusReads);
    });
    // And everything drawn from what is IN the domain: taking a side writes
    // the file on disk and the engine re-indexes it, so a tree, a listing and
    // an engram count from before the write are stale the moment it lands.
    await waitFor(() => {
      const [after, afterListings, afterListed] = contentReads();
      expect(after).toBeGreaterThan(trees);
      expect(afterListings).toBeGreaterThan(listings);
      expect(afterListed).toBeGreaterThan(listed);
    });
  });

  it("keeps a refused resolve in the dialog rather than swallowing it", async () => {
    serve({
      "/domains/eng/sync/conflicts/abc12345/resolve": () => {
        throw new ApiProblem(409, "conflict", "GitHub is not connected");
      },
    });

    renderApp("/d/eng");
    const dialog = await openConflictDialog();

    await userEvent.click(
      await within(dialog).findByRole("button", { name: "Take theirs" }),
    );

    // The dialog stays with the refusal on it: nothing was settled, and a
    // dialog that closed would leave the reader guessing whether it was.
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "GitHub is not connected",
    );
    expect(
      within(dialog).getByRole("button", { name: "Take theirs" }),
    ).toBeInTheDocument();
  });

  it("says what the detail read refused rather than drawing two empty sides", async () => {
    serve({
      "/domains/eng/sync/conflicts/abc12345": () => {
        throw new ApiProblem(
          403,
          "forbidden",
          "this instance is serving read-only",
        );
      },
    });

    renderApp("/d/eng");
    const dialog = await openConflictDialog();

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "this instance is serving read-only",
    );
    // Nothing to choose between, so nothing that offers to choose.
    expect(within(dialog).queryByRole("button", { name: "Take theirs" })).toBe(
      null,
    );
  });
});
