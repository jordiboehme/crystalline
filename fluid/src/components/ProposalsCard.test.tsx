/**
 * The proposals card: one row per proposal with where it stands on the origin,
 * the review's own verdict and the feedback behind it, and a withdraw that asks
 * first and offers to put the shared files back.
 *
 * Mounted through the domain screen rather than in isolation, because two of
 * the things under test are compositional: the card is admin-only, and it reads
 * the status the sync card already asked for rather than asking again.
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
 * The sync status in the shape the real route sends: the proposals themselves
 * rather than a count of them, each carrying its review standing and feedback.
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
    open_proposals: [
      {
        number: 4,
        url: "https://github.com/acme/knowledge/pull/4",
        title: "Refine 2 engrams in eng",
        // The engine's own casing: the three pre-existing states are
        // PascalCase and only the withdrawn one is lowercase.
        status: "Open",
        review_state: "changes_requested",
        amended_upstream: false,
        feedback: [
          {
            author: "ana",
            body: "needs a source",
            path: "notes/a.md",
            line: 12,
            submitted_at: "2026-08-21T10:00:00Z",
            kind: "review_comment",
          },
        ],
        updated_at: "2026-08-21T10:05:00Z",
      },
    ],
    declined_proposals: [],
    conflicts: [],
    stack_number: null,
    stack_wedged: [],
    repair_pending: false,
    stack_link_pending: false,
    ...overrides,
  };
}

/** One open layer, in chain order wherever it is put in the list. */
function openProposal(number: number, title: string) {
  return {
    number,
    url: `https://github.com/acme/knowledge/pull/${String(number)}`,
    title,
    status: "Open",
    review_state: null,
    amended_upstream: false,
    feedback: [],
    updated_at: null,
  };
}

/** Two open layers, bottom first, the way the report orders a chain. */
function stackedProposals() {
  return [
    openProposal(4, "Refine 2 engrams in eng"),
    openProposal(7, "One more pass on the routing"),
  ];
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
 * Every read a revert can invalidate, counted at once: the folders the
 * navigation walks, the listing the screen pages and the domain listing every
 * sidebar, card and switcher draws from.
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

/** The card itself, once the status behind it has landed. */
async function proposalsCard(): Promise<HTMLElement> {
  return screen.findByRole("region", { name: "Proposals" });
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the proposals card", () => {
  it("shows each proposal with its review standing and feedback", async () => {
    serve();

    renderApp("/d/eng");
    const card = await proposalsCard();

    const link = within(card).getByRole("link", {
      name: "Refine 2 engrams in eng",
    });
    expect(link).toHaveAttribute(
      "href",
      "https://github.com/acme/knowledge/pull/4",
    );
    // The review's verdict, in words rather than in the wire's underscore.
    expect(within(card).getByText("changes requested")).toBeInTheDocument();
    // Nothing was amended behind this proposal's back, so nothing says so.
    expect(within(card).queryByText(/amended upstream/i)).toBeNull();

    // The feedback is there to be read, not spread over the card by default:
    // a proposal with a review thread would bury every other row.
    expect(within(card).queryByText("needs a source")).toBeNull();
    await userEvent.click(
      within(card).getByRole("button", { name: /feedback/i }),
    );

    expect(within(card).getByText("needs a source")).toBeInTheDocument();
    // Where the comment is anchored, so a reader knows what it is about
    // without opening the proposal.
    expect(within(card).getByText(/notes\/a\.md:12/)).toBeInTheDocument();
  });

  it("will not link a proposal whose url is not a web address", async () => {
    serve({
      "/domains/eng/sync": () =>
        syncResponse({
          open_proposals: [
            {
              number: 4,
              url: "javascript:alert(1)",
              title: "Refine 2 engrams in eng",
              status: "Open",
              review_state: null,
              amended_upstream: false,
              feedback: [],
              updated_at: null,
            },
          ],
        }),
    });

    renderApp("/d/eng");
    const card = await proposalsCard();

    // The url comes from the forge rather than from this app, and a
    // self-hosted one is a machine somebody else administers. A title that
    // cannot be followed anywhere is still worth reading; a link that runs
    // something on press is not worth having.
    expect(within(card).queryByRole("link")).toBeNull();
    expect(
      within(card).getByText("Refine 2 engrams in eng"),
    ).toBeInTheDocument();
  });

  it("says when a reviewer moved the proposal branch", async () => {
    serve({
      "/domains/eng/sync": () =>
        syncResponse({
          open_proposals: [
            {
              number: 4,
              url: "https://github.com/acme/knowledge/pull/4",
              title: "Refine 2 engrams in eng",
              status: "Open",
              review_state: null,
              amended_upstream: true,
              feedback: [],
              updated_at: null,
            },
          ],
        }),
    });

    renderApp("/d/eng");
    const card = await proposalsCard();

    // The one fact a sharer has to know before sharing again: the branch under
    // this proposal is no longer only theirs.
    expect(within(card).getByText(/amended upstream/i)).toBeVisible();
    // No review has been submitted, so no verdict is invented for one.
    expect(within(card).queryByText("changes requested")).toBeNull();
    // And no feedback list to expand when there is no feedback.
    expect(
      within(card).queryByRole("button", { name: /feedback/i }),
    ).toBeNull();
  });

  it("lists the declined proposals beside the open ones", async () => {
    serve({
      "/domains/eng/sync": () =>
        syncResponse({
          declined_proposals: [
            {
              number: 2,
              url: "https://github.com/acme/knowledge/pull/2",
              title: "An idea the team turned down",
              status: "Declined",
              review_state: null,
              feedback: [],
              updated_at: "2026-08-19T09:00:00Z",
            },
          ],
        }),
    });

    renderApp("/d/eng");
    const card = await proposalsCard();

    expect(
      within(card).getByRole("link", { name: "An idea the team turned down" }),
    ).toBeVisible();
    // The two lists are one list here, and the status is what tells them
    // apart: a declined proposal wears its own word rather than reading as
    // one more open piece of work.
    expect(within(card).getByText("declined")).toBeVisible();
    expect(within(card).getByText("open")).toBeVisible();
  });

  it("says where each open layer sits, and which stack they are on", async () => {
    serve({
      "/domains/eng/sync": () =>
        syncResponse({
          open_proposals: stackedProposals(),
          stack_number: 42,
        }),
    });

    renderApp("/d/eng");
    const card = await proposalsCard();

    // Bottom-up, the way the chain is reviewed and the way the report orders
    // it: the first row is the layer everything else sits on.
    const rows = within(card).getAllByRole("listitem");
    expect(rows[0]).toHaveTextContent("Refine 2 engrams in eng");
    expect(rows[0]).toHaveTextContent("layer 1 of 2");
    expect(rows[1]).toHaveTextContent("One more pass on the routing");
    expect(rows[1]).toHaveTextContent("layer 2 of 2");
    // And the chain itself, named once rather than per row.
    expect(within(card).getByText("stack #42")).toBeVisible();
  });

  it("says nothing about layers when only one proposal is open", async () => {
    serve({ "/domains/eng/sync": () => syncResponse({ stack_number: 42 }) });

    renderApp("/d/eng");
    const card = await proposalsCard();

    // A lone proposal stands in no chain a reader needs told about: no
    // "layer 1 of 1" noise, and no stack to name either.
    expect(within(card).queryByText(/layer 1 of 1/i)).toBeNull();
    expect(within(card).queryByText(/^stack #/)).toBeNull();
  });

  it("says the link is pending rather than naming a stack it has no number for", async () => {
    serve({
      "/domains/eng/sync": () =>
        syncResponse({
          open_proposals: stackedProposals(),
          // Every layer exists; the call that groups them on the forge has
          // not landed yet.
          stack_number: null,
          stack_link_pending: true,
        }),
    });

    renderApp("/d/eng");
    const card = await proposalsCard();

    // The positions are real, so they are drawn.
    expect(within(card).getByText("layer 2 of 2")).toBeVisible();
    // The number is not, so nothing anywhere says "stack #".
    expect(within(card).queryByText(/stack #/)).toBeNull();
    expect(within(card).getByText(/stack link pending/i)).toBeVisible();
  });

  it("names the declined layer a wedged chain is stuck behind", async () => {
    serve({
      "/domains/eng/sync": () =>
        syncResponse({
          open_proposals: stackedProposals(),
          stack_number: 42,
          stack_wedged: [3],
          repair_pending: true,
        }),
    });

    renderApp("/d/eng");
    const card = await proposalsCard();

    // The number is what a reader acts on, and the sentence says which two
    // verbs act on it: a wedged chain cannot grow until one of them runs.
    expect(
      within(card).getByText(
        "Stack wedged by #3 - withdraw it or share again to repair the chain.",
      ),
    ).toBeVisible();
    // And the debt the next write settles by itself.
    expect(
      within(card).getByText(
        "Repair pending - the next share or withdraw finishes it.",
      ),
    ).toBeVisible();
  });

  it("warns before withdrawing a layer that is carrying others", async () => {
    serve({
      "/domains/eng/sync": () =>
        syncResponse({
          open_proposals: stackedProposals(),
          stack_number: 42,
        }),
    });

    renderApp("/d/eng");
    const card = await proposalsCard();

    // The bottom layer: closing it rebuilds everything above it, which is
    // work already in front of reviewers.
    await userEvent.click(
      within(card).getAllByRole("button", {
        name: "Withdraw",
      })[0] as HTMLElement,
    );
    const dialog = await screen.findByRole("dialog", { name: /withdraw/i });
    expect(
      within(dialog).getByText("Closes #4 and re-bases 1 layer above it."),
    ).toBeVisible();
  });

  it("says nothing about re-basing when the top layer is the one going", async () => {
    serve({
      "/domains/eng/sync": () =>
        syncResponse({
          open_proposals: stackedProposals(),
          stack_number: 42,
        }),
    });

    renderApp("/d/eng");
    const card = await proposalsCard();

    const buttons = within(card).getAllByRole("button", { name: "Withdraw" });
    await userEvent.click(buttons[buttons.length - 1] as HTMLElement);
    const dialog = await screen.findByRole("dialog", { name: /withdraw/i });
    // Nothing sits on the top layer, so nothing is re-based and nothing is
    // warned about.
    expect(within(dialog).queryByText(/re-bases/i)).toBeNull();
  });

  it("says which files a revert could not put back", async () => {
    serve({
      "/domains/eng/sync/proposals/4/withdraw": (_path, init) =>
        init?.method === "POST"
          ? {
              number: 4,
              closed: true,
              status: "withdrawn",
              restored: ["notes/a.md"],
              deleted: [],
              skipped_diverged: [],
              // No reachable copy of what this looked like before the share.
              skipped_reverts: ["notes/c.md"],
            }
          : null,
    });

    renderApp("/d/eng");
    const card = await proposalsCard();

    await userEvent.click(
      within(card).getByRole("button", { name: "Withdraw" }),
    );
    const dialog = await screen.findByRole("dialog", { name: /withdraw/i });
    await userEvent.click(
      within(dialog).getByLabelText("Restore shared files"),
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Withdraw proposal" }),
    );

    // The dialog closes on a withdraw that landed, so what it could not do
    // lands on the row rather than under a panel nothing can read.
    expect(await within(card).findByRole("status")).toHaveTextContent(
      "Could not restore: notes/c.md",
    );
  });

  it("withdraws through the confirm dialog, with the revert checkbox", async () => {
    const withdrawn = vi.fn(() => ({
      number: 4,
      closed: true,
      status: "withdrawn",
      restored: ["notes/a.md"],
      deleted: [],
      skipped_diverged: [],
    }));
    serve({
      "/domains/eng/sync/proposals/4/withdraw": (_path, init) =>
        init?.method === "POST" ? withdrawn() : null,
    });

    renderApp("/d/eng");
    const card = await proposalsCard();
    const statusReads = requested().filter(
      (path) => path === "/domains/eng/sync",
    ).length;
    const [trees, listings, listed] = contentReads();

    await userEvent.click(
      within(card).getByRole("button", { name: "Withdraw" }),
    );
    // The first press only asks: closing somebody else's review thread is not
    // something a single click gets to do.
    expect(withdrawn).not.toHaveBeenCalled();

    const dialog = await screen.findByRole("dialog", { name: /withdraw/i });
    await userEvent.click(
      within(dialog).getByLabelText("Restore shared files"),
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Withdraw proposal" }),
    );

    await waitFor(() => {
      expect(withdrawn).toHaveBeenCalled();
    });
    // The checkbox is the request's `revert` flag and nothing else.
    expect(sentBody("/domains/eng/sync/proposals/4/withdraw", "POST")).toEqual({
      revert: true,
    });
    // The proposal this card draws is no longer open, so the status behind it
    // is asked again rather than left showing what was withdrawn.
    await waitFor(() => {
      expect(
        requested().filter((path) => path === "/domains/eng/sync").length,
      ).toBeGreaterThan(statusReads);
    });
    // And the revert restored a file, which re-indexed the domain server-side:
    // everything drawn from what is in the domain is read again rather than
    // left showing the tree, the list and the count from before the restore.
    await waitFor(() => {
      const [after, afterListings, afterListed] = contentReads();
      expect(after).toBeGreaterThan(trees);
      expect(afterListings).toBeGreaterThan(listings);
      expect(afterListed).toBeGreaterThan(listed);
    });
  });

  it("sends no revert when the checkbox is left alone", async () => {
    const withdrawn = vi.fn(() => ({
      number: 4,
      closed: true,
      status: "withdrawn",
      restored: [],
      deleted: [],
      skipped_diverged: [],
    }));
    serve({
      "/domains/eng/sync/proposals/4/withdraw": (_path, init) =>
        init?.method === "POST" ? withdrawn() : null,
    });

    renderApp("/d/eng");
    const card = await proposalsCard();
    const statusReads = requested().filter(
      (path) => path === "/domains/eng/sync",
    ).length;
    const before = contentReads();

    await userEvent.click(
      within(card).getByRole("button", { name: "Withdraw" }),
    );
    const dialog = await screen.findByRole("dialog", { name: /withdraw/i });
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Withdraw proposal" }),
    );

    await waitFor(() => {
      expect(withdrawn).toHaveBeenCalled();
    });
    // Restoring files is the extra thing somebody asks for, never the default:
    // a withdraw that silently rewrote the working tree would be a surprise.
    expect(sentBody("/domains/eng/sync/proposals/4/withdraw", "POST")).toEqual({
      revert: false,
    });
    // The counterweight to the test above: this withdraw moved no file, so
    // nothing about the domain's contents is asked again. The status is - it
    // is what lists the proposal - and waiting for that is what makes the
    // three counts below a settled answer rather than a race.
    await waitFor(() => {
      expect(
        requested().filter((path) => path === "/domains/eng/sync").length,
      ).toBeGreaterThan(statusReads);
    });
    expect(contentReads()).toEqual(before);
  });

  it("keeps a refused withdraw on the row rather than swallowing it", async () => {
    serve({
      "/domains/eng/sync/proposals/4/withdraw": () => {
        throw new ApiProblem(
          409,
          "conflict",
          "GitHub is disabled on this instance",
        );
      },
    });

    renderApp("/d/eng");
    const card = await proposalsCard();

    await userEvent.click(
      within(card).getByRole("button", { name: "Withdraw" }),
    );
    const dialog = await screen.findByRole("dialog", { name: /withdraw/i });
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Withdraw proposal" }),
    );

    expect(await within(card).findByRole("alert")).toHaveTextContent(
      "GitHub is disabled on this instance",
    );
  });

  it("reads the status the sync card already asked for", async () => {
    serve();

    renderApp("/d/eng");
    await proposalsCard();
    await within(await screen.findByRole("main")).findByRole("region", {
      name: "Team sync",
    });

    // Two cards, one fetch: they share the query key and the fetcher, so the
    // second one costs a cache read rather than a second round trip.
    expect(
      requested().filter((path) => path === "/domains/eng/sync"),
    ).toHaveLength(1);
  });

  it("draws nothing at all for a domain with no origin", async () => {
    serve({
      "/domains/eng/sync": () => {
        throw new ApiProblem(
          404,
          "not found",
          "domain 'eng' has no team origin",
        );
      },
    });

    renderApp("/d/eng");
    await screen.findByRole("heading", { name: "eng" });

    // A domain with no origin has no proposals to have, which is not a failure
    // to report.
    expect(screen.queryByRole("region", { name: "Proposals" })).toBeNull();
    expect(screen.queryByText(/no team origin/)).toBeNull();
  });

  it("keeps the card, and the way to share, with no proposals on it", async () => {
    serve({
      "/domains/eng/sync": () =>
        syncResponse({ open_proposals: [], declined_proposals: [] }),
    });

    renderApp("/d/eng");
    const card = await proposalsCard();

    // An empty list is a state worth drawing, unlike a domain with no origin:
    // the way to make a proposal is this card's header, and a card that
    // vanished when the list emptied would take it away exactly when there is
    // something to share.
    expect(within(card).getByText("No open proposals.")).toBeVisible();
    expect(
      within(card).getByRole("button", { name: "Share changes" }),
    ).toBeVisible();
    expect(within(card).queryByRole("listitem")).toBeNull();
  });

  it("draws no proposals below admin", async () => {
    serve({
      "/auth/me": () => meResponse({ user: userFixture({ role: "editor" }) }),
    });

    renderApp("/d/eng");
    await screen.findByRole("heading", { name: "eng" });

    // The route behind it is admin-only, so an editor's screen knocks on
    // nothing it would be refused at.
    await waitFor(() => {
      expect(
        requested().some((path) => path.startsWith("/domains/eng/sync")),
      ).toBe(false);
    });
    expect(screen.queryByRole("region", { name: "Proposals" })).toBeNull();
  });
});
