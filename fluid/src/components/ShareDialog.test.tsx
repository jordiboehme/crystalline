/**
 * Sharing from the browser: what a share would do, read before anybody commits
 * to doing it, and the outcome read back in place afterwards.
 *
 * Mounted through the domain screen rather than in isolation, for the same
 * reason the proposals card is: the button that opens this dialog is admin-only
 * and lives on that card, and a dialog tested on its own would pass while
 * nothing on the screen could open it.
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
 * The sync status in the shape the real route sends. A team domain with one
 * open proposal, which is what puts the card on the screen; the share button
 * lives in its header either way.
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
        status: "Open",
        review_state: null,
        amended_upstream: false,
        feedback: [],
        updated_at: null,
      },
    ],
    declined_proposals: [],
    conflicts: [],
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
      ...routes,
    }),
  );
}

/** Every path the app asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => call[0]);
}

/** Whether one element is drawn before another, in document order. */
function precedes(before: Element, after: Element): boolean {
  return (
    (before.compareDocumentPosition(after) &
      Node.DOCUMENT_POSITION_FOLLOWING) !==
    0
  );
}

/** `count` changes of one kind, named apart so each is findable. */
function changesOf(kind: string, count: number, prefix: string) {
  return Array.from({ length: count }, (_unused, index) => ({
    path: `notes/${prefix}-${String(index)}.md`,
    kind,
  }));
}

/** How many times a route was asked for, exactly. */
function reads(path: string): number {
  return requested().filter((asked) => asked === path).length;
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

/** Open the dialog off the card's header button, once the card is up. */
async function openShareDialog(): Promise<HTMLElement> {
  const card = await screen.findByRole("region", { name: "Proposals" });
  await userEvent.click(
    within(card).getByRole("button", { name: "Share changes" }),
  );
  return screen.findByRole("dialog", { name: /share/i });
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the share dialog", () => {
  it("previews the action and shares with an edited title", async () => {
    const shared = vi.fn(() => ({
      outcome: "updated",
      proposal: { number: 4, url: "https://github.com/acme/knowledge/pull/4" },
    }));
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "update",
        number: 4,
        url: "https://github.com/acme/knowledge/pull/4",
        effective_title: "Refine 1 engram in eng",
        changes: [{ path: "notes/a.md", kind: "modified" }],
      }),
      "/domains/eng/sync/share": (_path, init) =>
        init?.method === "POST" ? shared() : null,
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();
    const statusReads = reads("/domains/eng/sync");
    const planReads = reads("/domains/eng/sync/changes");

    // The action line names the update rather than promising a new proposal.
    expect(
      await within(dialog).findByText(/updates proposal #4/i),
    ).toBeInTheDocument();
    // What would go into it, by name: a share nobody can see the contents of
    // is a share nobody can decide about.
    expect(within(dialog).getByText("notes/a.md")).toBeInTheDocument();

    // The title arrives prefilled and editable.
    const title = within(dialog).getByLabelText("Title");
    expect(title).toHaveValue("Refine 1 engram in eng");
    await userEvent.clear(title);
    await userEvent.type(title, "Sharper title");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Share" }),
    );

    await waitFor(() => {
      expect(shared).toHaveBeenCalled();
    });
    // The author's own words reach the server; an untouched description is
    // left out entirely, so the engine writes the body it would have written.
    expect(sentBody("/domains/eng/sync/share", "POST")).toEqual({
      title: "Sharper title",
    });

    // The outcome is surfaced where the decision was made, rather than the
    // dialog closing on nothing.
    expect(
      await within(dialog).findByText(/updated proposal #4/i),
    ).toBeInTheDocument();
    // And the line that said what a share WOULD do goes with it. Left
    // standing, the header would sit above the outcome telling somebody their
    // share is still about to happen, which is the one thing the dialog is
    // there to settle.
    expect(within(dialog).queryByText(/sharing updates/i)).toBeNull();
    expect(within(dialog).getByText("Done.")).toBeInTheDocument();
    // And the card behind it reads the status again: the proposal it lists
    // just moved.
    await waitFor(() => {
      expect(reads("/domains/eng/sync")).toBeGreaterThan(statusReads);
    });
    // The plan does not. Its key sits under the `["domains"]` prefix the
    // domain listing is invalidated by, so a plan query left enabled would
    // answer that invalidation with a second `GET /sync/changes` - a route
    // that pulls the origin - to re-plan a share that already happened, and a
    // refetch that then failed would put the planning-error line over an
    // outcome saying the share landed. Waiting for the status above is what
    // makes this a settled answer rather than a race: both invalidations are
    // fired from the one success handler.
    expect(reads("/domains/eng/sync/changes")).toBe(planReads);
  });

  it("sends no title when the prefilled one was left alone", async () => {
    const shared = vi.fn(() => ({
      outcome: "proposed",
      number: 7,
      url: "https://github.com/acme/knowledge/pull/7",
    }));
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "create",
        effective_title: "Share 2 new engrams from eng",
        changes: [
          { path: "notes/a.md", kind: "added" },
          { path: "notes/b.md", kind: "added" },
        ],
      }),
      "/domains/eng/sync/share": (_path, init) =>
        init?.method === "POST" ? shared() : null,
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    expect(
      await within(dialog).findByText(/opens a new proposal/i),
    ).toBeInTheDocument();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Share" }),
    );

    await waitFor(() => {
      expect(shared).toHaveBeenCalled();
    });
    // The prefill is what the server would have generated anyway, and sending
    // it back as an explicit title would retitle a proposal nobody asked to
    // rename. An untouched field means "your title", not "this one".
    expect(sentBody("/domains/eng/sync/share", "POST")).toEqual({});
    expect(
      await within(dialog).findByText(/opened proposal #7/i),
    ).toBeInTheDocument();
  });

  it("sends the description somebody wrote, trimmed", async () => {
    const shared = vi.fn(() => ({
      outcome: "proposed",
      number: 7,
      url: "https://github.com/acme/knowledge/pull/7",
    }));
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "create",
        effective_title: "Share 1 new engram from eng",
        changes: [{ path: "notes/a.md", kind: "added" }],
      }),
      "/domains/eng/sync/share": (_path, init) =>
        init?.method === "POST" ? shared() : null,
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    await userEvent.type(
      await within(dialog).findByLabelText("Description"),
      "  Sharper wording on the routing rules.  ",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Share" }),
    );

    await waitFor(() => {
      expect(shared).toHaveBeenCalled();
    });
    // The body of the proposal is the one thing here nobody else can write,
    // so it travels; the whitespace around it does not, and an untouched
    // title still stays behind.
    expect(sentBody("/domains/eng/sync/share", "POST")).toEqual({
      description: "Sharper wording on the routing rules.",
    });
  });

  it("names the layer a new proposal would stack on", async () => {
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "stack",
        effective_title: "Refine 1 engram in eng",
        changes: [{ path: "notes/a.md", kind: "modified" }],
        top_number: 4,
        top_title: "Refine 2 engrams in eng",
      }),
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    // A stack is a proposal of its own, so the share is offered rather than
    // refused, and the line says what it would sit on.
    expect(
      await within(dialog).findByText(
        "Will stack a new proposal on top of #4.",
      ),
    ).toBeInTheDocument();
    await waitFor(() => {
      expect(
        within(dialog).getByRole("button", { name: "Share" }),
      ).toBeEnabled();
    });
  });

  it("offers the share on an amend plan, and says what it rebuilds", async () => {
    const shared = vi.fn(() => ({
      outcome: "updated",
      proposal: {
        number: 4,
        url: "https://github.com/acme/knowledge/pull/4",
        stack_number: 42,
        stack_position: [1, 2],
      },
    }));
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "amend",
        effective_title: "Refine 1 engram in eng",
        changes: [{ path: "notes/a.md", kind: "modified" }],
        number: 4,
        url: "https://github.com/acme/knowledge/pull/4",
        layers_above: 1,
      }),
      "/domains/eng/sync/share": (_path, init) =>
        init?.method === "POST" ? shared() : null,
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    // An amend puts a fresh commit on a proposal that already exists, so it is
    // shareable like an update - and it says how much work above it would be
    // rebuilt, which is the whole difference from amending the top layer.
    expect(
      await within(dialog).findByText(
        "Sharing amends proposal #4 and re-bases 1 layer above it.",
      ),
    ).toBeInTheDocument();
    await waitFor(() => {
      expect(
        within(dialog).getByRole("button", { name: "Share" }),
      ).toBeEnabled();
    });
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Share" }),
    );

    await waitFor(() => {
      expect(shared).toHaveBeenCalled();
    });
    // The server already planned this target, so nothing names it again: the
    // number travels only when somebody picked a layer of their own.
    expect(sentBody("/domains/eng/sync/share", "POST")).toEqual({});
    expect(
      await within(dialog).findByText(
        "Updated proposal #4, layer 1 of 2 on stack #42.",
      ),
    ).toBeInTheDocument();
  });

  it("offers the open layers to amend, and names the one that was chosen", async () => {
    const shared = vi.fn(() => ({
      outcome: "updated",
      proposal: { number: 4, url: "https://github.com/acme/knowledge/pull/4" },
    }));
    serve({
      "/domains/eng/sync": () =>
        syncResponse({
          open_proposals: [
            {
              number: 4,
              url: "https://github.com/acme/knowledge/pull/4",
              title: "Refine 2 engrams in eng",
              status: "Open",
              review_state: "changes_requested",
              amended_upstream: false,
              feedback: [],
              updated_at: null,
            },
            {
              number: 7,
              url: "https://github.com/acme/knowledge/pull/7",
              title: "One more pass on the routing",
              status: "Open",
              review_state: null,
              amended_upstream: false,
              feedback: [],
              updated_at: null,
            },
          ],
          stack_number: 42,
        }),
      "/domains/eng/sync/changes": () => ({
        action: "stack",
        effective_title: "Refine 1 engram in eng",
        changes: [{ path: "notes/a.md", kind: "modified" }],
        top_number: 7,
        top_title: "One more pass on the routing",
      }),
      "/domains/eng/sync/share": (_path, init) =>
        init?.method === "POST" ? shared() : null,
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    // Stacking on top is the default: acting on a layer's review feedback is
    // the deliberate choice, and it is made by naming the layer.
    const select = await within(dialog).findByLabelText("Proposal");
    expect(select).toHaveValue("");
    await userEvent.selectOptions(select, "4");

    // The one thing somebody amending a lower layer has to know, because the
    // layer above it would simply overwrite the change.
    expect(
      within(dialog).getByText(
        "Changes to files a higher layer already touched belong in that layer instead.",
      ),
    ).toBeVisible();

    await userEvent.click(
      within(dialog).getByRole("button", { name: "Share" }),
    );
    await waitFor(() => {
      expect(shared).toHaveBeenCalled();
    });
    // The chosen layer is the whole of what the choice sends: the prefilled
    // title still stays behind.
    expect(sentBody("/domains/eng/sync/share", "POST")).toEqual({
      proposal: 4,
    });
  });

  it("offers the newest layer first, under the default that stacks", async () => {
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
              amended_upstream: false,
              feedback: [],
              updated_at: null,
            },
            {
              number: 7,
              url: "https://github.com/acme/knowledge/pull/7",
              title: "One more pass on the routing",
              status: "Open",
              review_state: null,
              amended_upstream: false,
              feedback: [],
              updated_at: null,
            },
          ],
        }),
      "/domains/eng/sync/changes": () => ({
        action: "stack",
        effective_title: "Refine 1 engram in eng",
        changes: [{ path: "notes/a.md", kind: "modified" }],
        top_number: 7,
        top_title: "One more pass on the routing",
      }),
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();
    const select = await within(dialog).findByLabelText("Proposal");

    // The report orders a chain bottom first, and a picker that repeated that
    // order would put the layer somebody just shared - the one their review
    // feedback is about - at the far end of the list. The layer that is
    // stacked on is the layer most likely to be amended, so it comes first,
    // under the default that stacks a new one over the top of it.
    expect(
      within(select)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual([
      "New proposal (stack on top)",
      "Amend #7 - One more pass on the routing",
      "Amend #4 - Refine 2 engrams in eng",
    ]);
  });

  it("groups a long change list by kind and keeps the rest one press away", async () => {
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "create",
        effective_title: "Share 11 engrams from eng",
        changes: [
          ...changesOf("added", 3, "new"),
          ...changesOf("modified", 7, "mod"),
          { path: "notes/gone.md", kind: "deleted" },
        ],
      }),
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    // A sweep shares dozens to hundreds of files at once. The count per kind
    // is the shape of the share; the paths are the detail behind it.
    expect(await within(dialog).findByText("Added 3")).toBeVisible();
    expect(within(dialog).getByText("Modified 7")).toBeVisible();
    expect(within(dialog).getByText("Deleted 1")).toBeVisible();

    // A group under the cap is whole, so nothing offers to expand it.
    expect(within(dialog).getByText("notes/new-2.md")).toBeVisible();
    // The one over it shows its first few and counts the rest.
    expect(within(dialog).getByText("notes/mod-4.md")).toBeVisible();
    expect(within(dialog).queryByText("notes/mod-5.md")).toBeNull();

    const more = within(dialog).getByRole("button", { name: "and 2 more" });
    expect(more).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(more);

    // Nothing is lost: the whole group is one press away, inside the same
    // scrolling box rather than growing the dialog past the screen.
    expect(within(dialog).getByText("notes/mod-5.md")).toBeVisible();
    expect(within(dialog).getByText("notes/mod-6.md")).toBeVisible();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Show fewer" }),
    );
    expect(within(dialog).queryByText("notes/mod-5.md")).toBeNull();
  });

  it("badges each change with its kind letter and the word behind it", async () => {
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "create",
        effective_title: "Share 3 engrams from eng",
        changes: [
          { path: "notes/a.md", kind: "added" },
          { path: "notes/b.md", kind: "modified" },
          { path: "notes/c.md", kind: "deleted" },
        ],
      }),
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    // The source-control idiom: one letter per row, color behind it rather
    // than under it, and the word itself for anything that reads the page -
    // color is never the only thing carrying the meaning.
    const added = await within(dialog).findByRole("img", { name: "Added" });
    expect(added).toHaveTextContent("A");
    expect(
      within(dialog).getByRole("img", { name: "Modified" }),
    ).toHaveTextContent("M");
    expect(
      within(dialog).getByRole("img", { name: "Deleted" }),
    ).toHaveTextContent("D");
  });

  it("keeps a kind it has not been taught, as the word it arrived as", async () => {
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "create",
        effective_title: "Share 1 engram from eng",
        changes: [{ path: "notes/a.md", kind: "vaporized" }],
      }),
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    // A word this side has not been taught is somebody else's vocabulary
    // rather than a malformed one: it gets the neutral face and its own
    // initial, and the group still says what it is and how much of it there
    // is.
    expect(await within(dialog).findByText("vaporized 1")).toBeVisible();
    expect(
      within(dialog).getByRole("img", { name: "vaporized" }),
    ).toHaveTextContent("V");
  });

  it("asks which layer first, then says what the share would do", async () => {
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
              amended_upstream: false,
              feedback: [],
              updated_at: null,
            },
          ],
        }),
      "/domains/eng/sync/changes": () => ({
        action: "stack",
        effective_title: "Refine 1 engram in eng",
        changes: [{ path: "notes/a.md", kind: "modified" }],
        top_number: 4,
        top_title: "Refine 2 engrams in eng",
      }),
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    // The order the decision is actually made in: which layer this lands on
    // decides what the sentence under it says, the changes are what is being
    // landed, and the two fields are the wording somebody writes last.
    const select = await within(dialog).findByLabelText("Proposal");
    const line = within(dialog).getByText(
      "Will stack a new proposal on top of #4.",
    );
    const changes = within(dialog).getByText("Modified 1");
    const title = within(dialog).getByLabelText("Title");
    const description = within(dialog).getByLabelText("Description");

    expect(precedes(select, line)).toBe(true);
    expect(precedes(line, changes)).toBe(true);
    expect(precedes(changes, title)).toBe(true);
    expect(precedes(title, description)).toBe(true);

    // And the choice still rewrites the sentence rather than leaving the
    // server's own plan standing over a target somebody just changed.
    await userEvent.selectOptions(select, "4");
    expect(
      within(dialog).getByText("Sharing amends proposal #4."),
    ).toBeVisible();
  });

  it("says where the proposal it just opened sits in the chain", async () => {
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "stack",
        effective_title: "Refine 1 engram in eng",
        changes: [{ path: "notes/a.md", kind: "modified" }],
        top_number: 4,
        top_title: "Refine 2 engrams in eng",
      }),
      "/domains/eng/sync/share": (_path, init) =>
        init?.method === "POST"
          ? {
              outcome: "proposed",
              number: 7,
              url: "https://github.com/acme/knowledge/pull/7",
              stack_number: 42,
              stack_position: [2, 2],
            }
          : null,
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();
    await waitFor(() => {
      expect(
        within(dialog).getByRole("button", { name: "Share" }),
      ).toBeEnabled();
    });
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Share" }),
    );

    expect(
      await within(dialog).findByText(
        "Opened proposal #7, layer 2 of 2 on stack #42.",
      ),
    ).toBeInTheDocument();
  });

  it("names no stack number for a chain that is not linked yet", async () => {
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "stack",
        effective_title: "Refine 1 engram in eng",
        changes: [{ path: "notes/a.md", kind: "modified" }],
        top_number: 4,
        top_title: "Refine 2 engrams in eng",
      }),
      "/domains/eng/sync/share": (_path, init) =>
        init?.method === "POST"
          ? {
              outcome: "proposed",
              number: 7,
              url: "https://github.com/acme/knowledge/pull/7",
              // The layers all exist; the call that groups them has not
              // landed, so there is no number to name.
              stack_number: null,
              stack_position: [2, 2],
            }
          : null,
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();
    await waitFor(() => {
      expect(
        within(dialog).getByRole("button", { name: "Share" }),
      ).toBeEnabled();
    });
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Share" }),
    );

    expect(
      await within(dialog).findByText(
        "Opened proposal #7, layer 2 of 2 (stack link pending).",
      ),
    ).toBeInTheDocument();
  });

  it("offers no layer to amend when nothing is open", async () => {
    serve({
      "/domains/eng/sync": () => syncResponse({ open_proposals: [] }),
      "/domains/eng/sync/changes": () => ({
        action: "create",
        effective_title: "Share 1 new engram from eng",
        changes: [{ path: "notes/a.md", kind: "added" }],
      }),
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    await within(dialog).findByText(/opens a new proposal/i);
    // A choice between one thing is not a choice, and there is no layer to
    // amend at all here.
    expect(within(dialog).queryByLabelText("Proposal")).toBeNull();
  });

  it("disables the share with the reason when there is nothing to confirm", async () => {
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "conflicts_pending",
        count: 2,
        effective_title: "",
        changes: [],
      }),
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    await waitFor(() => {
      expect(
        within(dialog).getByRole("button", { name: "Share" }),
      ).toBeDisabled();
    });
    // With the count the report carried: "conflicts need settling" is advice,
    // and two of them is a size somebody can decide to sit down with now.
    expect(
      within(dialog).getByText("2 conflicts need settling before sharing."),
    ).toBeInTheDocument();
  });

  it("counts one conflict as one", async () => {
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "conflicts_pending",
        count: 1,
        effective_title: "",
        changes: [],
      }),
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    expect(
      await within(dialog).findByText(
        "1 conflict needs settling before sharing.",
      ),
    ).toBeInTheDocument();
  });

  it.each([
    {
      action: "nothing_to_share",
      plan: {},
      says: /the team already has all of this/i,
    },
    {
      action: "proposal_diverged",
      plan: {
        number: 4,
        url: "https://github.com/acme/knowledge/pull/4",
        branch: "crystalline/eng-20260821",
      },
      says: /a reviewer amended proposal #4/i,
    },
  ])(
    "refuses to offer a share on a $action plan, and says why",
    async ({ action, plan, says }) => {
      serve({
        "/domains/eng/sync/changes": () => ({
          action,
          effective_title: "",
          changes: [],
          ...plan,
        }),
      });

      renderApp("/d/eng");
      const dialog = await openShareDialog();

      // The sentence is the whole of the help here: neither of these is a
      // failure to report, and neither is something pressing Share could fix.
      expect(await within(dialog).findByText(says)).toBeInTheDocument();
      expect(
        within(dialog).getByRole("button", { name: "Share" }),
      ).toBeDisabled();
    },
  );

  it("keeps a refused share in the dialog rather than swallowing it", async () => {
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "create",
        effective_title: "Share 1 new engram from eng",
        changes: [{ path: "notes/a.md", kind: "added" }],
      }),
      "/domains/eng/sync/share": () => {
        throw new ApiProblem(409, "conflict", "GitHub is not connected");
      },
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    await waitFor(() => {
      expect(
        within(dialog).getByRole("button", { name: "Share" }),
      ).toBeEnabled();
    });
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Share" }),
    );

    // The dialog stays open with the refusal on it: the form still holds a
    // title somebody wrote, and closing it would throw that away.
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "GitHub is not connected",
    );
    expect(
      within(dialog).getByRole("button", { name: "Share" }),
    ).toBeInTheDocument();
  });

  it("says what a read-only instance refused rather than showing an empty plan", async () => {
    serve({
      "/domains/eng/sync/changes": () => {
        throw new ApiProblem(
          403,
          "forbidden",
          "this instance is serving read-only",
        );
      },
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "this instance is serving read-only",
    );
    // Nothing to share into, so nothing that offers to.
    expect(
      within(dialog).getByRole("button", { name: "Share" }),
    ).toBeDisabled();
  });
});
