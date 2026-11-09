/**
 * Settling a conflict from the browser: both sides read side by side, one of
 * them taken, or a text somebody wrote out of the two.
 *
 * Mounted through the domain screen rather than in isolation, for the same
 * reason the share dialog is: the only way in is the sync card's conflict list,
 * which is admin-only, and a dialog tested on its own would pass while nothing
 * on the screen could open it.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
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
import type { ConflictDialogProps } from "./ConflictDialog";
import { ConflictDialog } from "./ConflictDialog";

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
  it("lists a conflict that can be opened as a path, after a colon", async () => {
    serve();

    renderApp("/d/eng");
    const card = await screen.findByRole("region", { name: "Team sync" });

    // The count still leads, and the colon is what turns it into an
    // introduction to the paths under it. The two no-colon cases - a report
    // that carried a bare count, and one that listed paths with no id to
    // address them by - are pinned on the domain screen's own tests, and this
    // is the branch that has somewhere to go.
    expect(within(card).getByText("1 conflict to settle:")).toBeVisible();
    expect(
      within(card).getByRole("button", { name: "notes/a.md" }),
    ).toBeVisible();
  });

  it("draws at most eight paths and counts the rest in words", async () => {
    serve({
      "/domains/eng/sync": () =>
        syncResponse({
          conflicts: Array.from({ length: 10 }, (_unused, index) => ({
            id: `conflict-${String(index)}`,
            path: `notes/${String(index)}.md`,
            kind: "EditEdit",
            base_commit: "9f3c1a2",
            upstream_commit: "b2c3d4e",
            detected_at: "2026-08-21T09:00:00Z",
          })),
        }),
    });

    renderApp("/d/eng");
    const card = await screen.findByRole("region", { name: "Team sync" });

    // The sentence counts every one of them, because how far behind this copy
    // has fallen is the fact the card is there to state.
    expect(within(card).getByText("10 conflicts to settle:")).toBeVisible();
    // The list does not. A domain that drifted for a week would otherwise be a
    // wall of paths with everything under it pushed off the screen, and eight
    // is enough to start settling them one at a time.
    expect(
      within(card).getAllByRole("button", { name: /^notes\// }),
    ).toHaveLength(8);
    // What was left out is said rather than silently dropped, and as plain
    // text: there is nothing behind it to press.
    expect(within(card).getByText("and 2 more")).toBeVisible();
  });

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
    const [trees, listings, listed] = contentReads();

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
    // And the domain is read again even though this side of the choice wrote
    // no file at all: the engine re-syncs after every resolve, keeping this
    // copy's own version included, so the tree, the listing and the count are
    // all about to be stale whichever way the conflict was settled.
    await waitFor(() => {
      const [after, afterListings, afterListed] = contentReads();
      expect(after).toBeGreaterThan(trees);
      expect(afterListings).toBeGreaterThan(listings);
      expect(afterListed).toBeGreaterThan(listed);
    });
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

  it("keeps a hand-written merge when the editor is toggled shut", async () => {
    serve();

    renderApp("/d/eng");
    const dialog = await openConflictDialog();

    const toggle = await within(dialog).findByRole("button", {
      name: "Edit merged",
    });
    await userEvent.click(toggle);
    const editor = within(dialog).getByLabelText("Merged content");
    await userEvent.clear(editor);
    await userEvent.type(editor, "reconciled text");

    // Closing the editor hides it and nothing else. The toggle sits one button
    // away from "Save merged", so a misclick here must not be the thing that
    // deletes a merge somebody wrote by hand.
    await userEvent.click(toggle);
    expect(within(dialog).queryByLabelText("Merged content")).toBeNull();
    await userEvent.click(toggle);

    expect(within(dialog).getByLabelText("Merged content")).toHaveValue(
      "reconciled text",
    );
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

/** One of this session's draft conflicts with its sides, as the route sends it. */
function draftSides(overrides: Record<string, unknown> = {}) {
  return {
    id: "d1234567",
    path: "notes/a.md",
    kind: "EditEdit",
    moved_from: null,
    other: null,
    base: "the start",
    local: "my draft text",
    upstream: "the team's new text",
    note: null,
    detected_at: null,
    draft: true,
    ...overrides,
  };
}

/**
 * Serve the two draft routes for `d1234567`, recording each resolve body;
 * every other path is a 404, the folder's routes included.
 */
function serveDraft(sides: Record<string, unknown> = draftSides()) {
  const resolved: unknown[] = [];
  apiMock.mockImplementation(async (path: string, init?: RequestInit) => {
    await Promise.resolve();
    if (path === "/domains/eng/drafts/conflicts/d1234567") {
      return sides;
    }
    if (
      path === "/domains/eng/drafts/conflicts/d1234567/resolve" &&
      init?.method === "POST"
    ) {
      resolved.push(
        typeof init.body === "string" ? JSON.parse(init.body) : init.body,
      );
      return {
        draft: true,
        kept: "mine",
        kept_copy: false,
        remaining: 0,
      };
    }
    throw new ApiProblem(404, "not found", `no stub for ${path}`);
  });
  return resolved;
}

/** The dialog for a draft conflict, mounted on its own. */
function mountDraft(
  draft: NonNullable<ConflictDialogProps["draft"]>,
  onClose: () => void = vi.fn(),
) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const view = render(
    <QueryClientProvider client={client}>
      <ConflictDialog
        domain="eng"
        conflictId="d1234567"
        draft={draft}
        onClose={onClose}
      />
    </QueryClientProvider>,
  );
  return { client, unmount: view.unmount };
}

const EDIT_EDIT = {
  kind: "EditEdit",
  movedFrom: null,
  other: null,
  file: false,
};

/** The accessible description a control points at, as one string. */
function hintOf(control: HTMLElement): string {
  return (control.getAttribute("aria-describedby") ?? "")
    .split(" ")
    .filter((id) => id !== "")
    .map((id) => document.getElementById(id)?.textContent ?? "")
    .join(" ");
}

describe("the conflict dialog for a draft", () => {
  it("uses the draft routes and the labels Your draft and The team's text", async () => {
    const resolved = serveDraft();
    const onClose = vi.fn();
    mountDraft(EDIT_EDIT, onClose);
    const dialog = await screen.findByRole("dialog", {
      name: "Settle conflict",
    });

    expect(await within(dialog).findByText("my draft text")).toBeVisible();
    expect(within(dialog).getByText("the team's new text")).toBeVisible();
    expect(within(dialog).getByText("Your draft")).toBeVisible();
    expect(within(dialog).getByText("The team's text")).toBeVisible();
    // The folder's words are not a draft's.
    expect(within(dialog).queryByText("Mine (local)")).toBeNull();
    expect(within(dialog).queryByText("Theirs (upstream)")).toBeNull();

    await userEvent.click(
      within(dialog).getByRole("button", { name: "Keep mine" }),
    );
    await waitFor(() => {
      expect(resolved).toEqual([{ resolution: "mine" }]);
    });
    await waitFor(() => {
      expect(onClose).toHaveBeenCalled();
    });
    // Never the folder's route, which answers 404 for a draft's id.
    expect(
      apiMock.mock.calls.some(([path]) => path.includes("/sync/conflicts")),
    ).toBe(false);
  });

  it("says the kind's sentence for each kind", async () => {
    const rows: [Record<string, unknown>, string][] = [
      [
        { kind: "EditEdit" },
        "You and the team changed the same lines of notes/a.md.",
      ],
      [
        { kind: "EditEdit", moved_from: "notes/old.md" },
        "You moved notes/old.md to notes/a.md, and the team changed it. The changes do not fit together.",
      ],
      [{ kind: "EditDelete" }, "You changed notes/a.md. The team deleted it."],
      [{ kind: "DeleteEdit" }, "You deleted notes/a.md. The team changed it."],
      [
        { kind: "AddAdd" },
        "You and the team both have a file at notes/a.md, with different text.",
      ],
      [
        { kind: "UnknownStart" },
        "This draft is from an older version, so we cannot tell what the team changed since. Compare the two texts.",
      ],
      [
        { kind: "AddressTaken", other: "notes/z.md" },
        "The team's file notes/z.md now uses the same address as your draft of notes/a.md.",
      ],
    ];
    for (const [over, sentence] of rows) {
      serveDraft(draftSides(over));
      const view = mountDraft({
        kind: String(over.kind),
        movedFrom: (over.moved_from as string | undefined) ?? null,
        other: (over.other as string | undefined) ?? null,
        file: false,
      });
      const dialog = await screen.findByRole("dialog", {
        name: "Settle conflict",
      });
      await waitFor(() => {
        expect(dialog).toHaveAccessibleDescription(sentence);
      });
      view.unmount();
    }
  });

  it("says what each choice does", async () => {
    serveDraft();
    mountDraft(EDIT_EDIT);
    const dialog = await screen.findByRole("dialog", {
      name: "Settle conflict",
    });
    const mine = await within(dialog).findByRole("button", {
      name: "Keep mine",
    });
    const theirs = within(dialog).getByRole("button", { name: "Take theirs" });

    expect(hintOf(mine)).toBe("Your text replaces the team's when you share.");
    expect(hintOf(theirs)).toBe("Your draft ends and your text is gone.");
    expect(
      within(dialog).getByRole("button", { name: "Edit merged" }),
    ).toBeEnabled();
  });

  it("says a copy is kept for an old draft", async () => {
    serveDraft(draftSides({ kind: "UnknownStart" }));
    mountDraft({ ...EDIT_EDIT, kind: "UnknownStart" });
    const dialog = await screen.findByRole("dialog", {
      name: "Settle conflict",
    });
    const theirs = await within(dialog).findByRole("button", {
      name: "Take theirs",
    });

    expect(hintOf(theirs)).toBe(
      "Your draft ends. A copy stays in your kept texts.",
    );
  });

  it("turns Keep mine off for a taken address with the hint", async () => {
    const resolved = serveDraft(
      draftSides({ kind: "AddressTaken", other: "notes/z.md" }),
    );
    mountDraft({ ...EDIT_EDIT, kind: "AddressTaken", other: "notes/z.md" });
    const dialog = await screen.findByRole("dialog", {
      name: "Settle conflict",
    });
    const mine = await within(dialog).findByRole("button", {
      name: "Keep mine",
    });

    expect(mine).toBeDisabled();
    expect(hintOf(mine)).toBe(
      "Change the address in Edit merged, or take theirs.",
    );
    // The two ways it does name stay open.
    expect(
      within(dialog).getByRole("button", { name: "Edit merged" }),
    ).toBeEnabled();
    expect(
      within(dialog).getByRole("button", { name: "Take theirs" }),
    ).toBeEnabled();
    await userEvent.click(mine);
    expect(resolved).toEqual([]);
  });

  it("hides Edit merged for a file", async () => {
    serveDraft(
      draftSides({
        path: "assets/deck.pdf",
        base: null,
        local: null,
        upstream: null,
        note: "A file has no text to compare. Keep yours or take the team's.",
        file: true,
      }),
    );
    mountDraft({ ...EDIT_EDIT, file: true });
    const dialog = await screen.findByRole("dialog", {
      name: "Settle conflict",
    });

    expect(
      await within(dialog).findByText(
        "A file cannot be merged here. Keep one of the two.",
      ),
    ).toBeVisible();
    expect(
      within(dialog).queryByRole("button", { name: "Edit merged" }),
    ).toBeNull();
    expect(
      within(dialog).getByRole("button", { name: "Keep mine" }),
    ).toBeEnabled();
    expect(
      within(dialog).getByRole("button", { name: "Take theirs" }),
    ).toBeEnabled();
    // The server's own sentence about the file, as it was sent.
    expect(
      within(dialog).getByText(
        "A file has no text to compare. Keep yours or take the team's.",
      ),
    ).toBeVisible();
    // A file's sides are not deletions.
    expect(within(dialog).queryByText("You deleted this file.")).toBeNull();
    expect(
      within(dialog).queryByText("The team deleted this file."),
    ).toBeNull();
  });

  it("says You deleted this file. and The team deleted this file. on an empty side", async () => {
    serveDraft(draftSides({ kind: "DeleteEdit", local: null }));
    const first = mountDraft({ ...EDIT_EDIT, kind: "DeleteEdit" });
    let dialog = await screen.findByRole("dialog", { name: "Settle conflict" });
    expect(
      await within(dialog).findByText("You deleted this file."),
    ).toBeVisible();
    expect(within(dialog).getByText("the team's new text")).toBeVisible();
    first.unmount();

    serveDraft(draftSides({ kind: "EditDelete", upstream: null }));
    mountDraft({ ...EDIT_EDIT, kind: "EditDelete" });
    dialog = await screen.findByRole("dialog", { name: "Settle conflict" });
    expect(
      await within(dialog).findByText("The team deleted this file."),
    ).toBeVisible();
    expect(within(dialog).getByText("my draft text")).toBeVisible();
  });

  it("shows the server's note about a kept copy before it closes", async () => {
    const onClose = vi.fn();
    apiMock.mockImplementation(async (path: string, init?: RequestInit) => {
      await Promise.resolve();
      if (path === "/domains/eng/drafts/conflicts/d1234567") {
        return draftSides({
          kind: "UnknownStart",
          file: true,
          local: null,
          upstream: null,
        });
      }
      if (init?.method === "POST") {
        return {
          draft: true,
          file: true,
          kept: "theirs",
          kept_copy: true,
          kept_note: "A copy of your file is kept. Find it in your kept list.",
          remaining: 0,
        };
      }
      throw new ApiProblem(404, "not found", path);
    });
    mountDraft({ ...EDIT_EDIT, kind: "UnknownStart", file: true }, onClose);
    const dialog = await screen.findByRole("dialog", {
      name: "Settle conflict",
    });

    await userEvent.click(
      await within(dialog).findByRole("button", { name: "Take theirs" }),
    );

    expect(
      await within(dialog).findByText(
        "A copy of your file is kept. Find it in your kept list.",
      ),
    ).toBeVisible();
    expect(onClose).not.toHaveBeenCalled();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Close" }),
    );
    expect(onClose).toHaveBeenCalled();
  });

  it("reads the draft list and the share plan again once settled", async () => {
    serveDraft();
    const { client } = mountDraft(EDIT_EDIT);
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const dialog = await screen.findByRole("dialog", {
      name: "Settle conflict",
    });

    await userEvent.click(
      await within(dialog).findByRole("button", { name: "Take theirs" }),
    );

    await waitFor(() => {
      const keys = invalidate.mock.calls.map((call) =>
        JSON.stringify(call[0]?.queryKey),
      );
      expect(keys).toContain(
        JSON.stringify(["domains", "eng", "draft-conflicts"]),
      );
      expect(keys).toContain(JSON.stringify(["share-plan", "eng"]));
      expect(keys).toContain(JSON.stringify(["domains", "eng", "sync"]));
    });
  });

  it("a dialog with no draft prop reads as before", async () => {
    serve();

    renderApp("/d/eng");
    const dialog = await openConflictDialog();

    // The folder's conflict keeps its own words and its own route.
    expect(await within(dialog).findByText("Mine (local)")).toBeVisible();
    expect(within(dialog).getByText("Theirs (upstream)")).toBeVisible();
    expect(dialog).toHaveAccessibleDescription(
      "notes/a.md - this copy and the team's changed the same engram.",
    );
    expect(
      within(dialog).getByRole("button", { name: "Keep mine" }),
    ).toBeEnabled();
    expect(
      within(dialog).getByRole("button", { name: "Edit merged" }),
    ).toBeEnabled();
    // Never a draft's sides or resolve. (The domain page's notice reads the
    // draft list beside the card; that is the notice's, not this dialog's.)
    expect(
      apiMock.mock.calls.some(([path]) =>
        path.startsWith("/domains/eng/drafts/conflicts/"),
      ),
    ).toBe(false);
  });
});
