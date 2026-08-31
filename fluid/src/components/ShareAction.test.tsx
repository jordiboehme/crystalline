/**
 * The top bar's share action: who is offered it, when it has anything to do,
 * and which of the two doors it opens.
 *
 * Mounted through the whole app rather than in isolation, the way the share
 * dialog's own tests are: what is under test here is a frame deciding from
 * three separate answers - who is asking, whether GitHub is on, and whether
 * anything is waiting - and a button rendered on its own would pass while
 * nothing on the screen could reach it.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { Role } from "../api/model";
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

/** A GitHub connection that is on and has a credential on file. */
function githubStatus(overrides: Record<string, unknown> = {}) {
  return {
    enabled: true,
    connected: true,
    user: "octo",
    token_store: "keychain",
    pending: null,
    error: null,
    ...overrides,
  };
}

/** One team domain's own report, in the shape the per-domain route sends. */
function syncResponse(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    mode: "github",
    repo: "acme/knowledge",
    branch: "main",
    behind: false,
    local_changes: 2,
    last_checked: "2026-08-21T08:00:00Z",
    probe_error: null,
    connection: { connected: true, user: "octo", token_store: "keychain" },
    open_proposals: [],
    declined_proposals: [],
    conflicts: [],
    ...overrides,
  };
}

/** The instance-wide summary, in the shape `GET /sync` sends. */
function summaryResponse(domains: Record<string, unknown>[]) {
  return {
    connection: { connected: true, user: "octo", token_store: "keychain" },
    domains,
    errors: [],
  };
}

/** One counted summary entry, zero everywhere but where it is told otherwise. */
function summaryEntry(
  domain: string,
  localChanges: number,
  overrides: Record<string, unknown> = {},
) {
  return {
    domain,
    mode: "github",
    repo: `acme/${domain}`,
    branch: "main",
    last_checked: "2026-08-21T08:00:00Z",
    local_changes: localChanges,
    open_proposals: 0,
    declined_proposals: 0,
    conflicts: 0,
    stack_wedged: [],
    repair_pending: false,
    stack_link_pending: false,
    ...overrides,
  };
}

/** An admin session, with whatever the screen under it needs beside it. */
function serve(routes: Record<string, Answer> = {}, role: Role = "admin") {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role }) }),
      "/domains": domainsResponse,
      "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
      ...routes,
    }),
  );
}

/** The routes the domain screen behind the frame reads for itself. */
function domainScreenRoutes(domain: string): Record<string, Answer> {
  return {
    [`/domains/${domain}/manifest`]: () => ({
      domain,
      markdown: `# ${domain}\n`,
    }),
    [`/domains/${domain}/tree`]: () => ({
      domain,
      path: "/",
      folders: [],
      engrams: [],
    }),
    [`/domains/${domain}/engrams`]: () => ({
      mode: "text",
      total: 0,
      page: 1,
      limit: 50,
      count: 0,
      hits: [],
    }),
    "/vocabulary": () => ({
      domain,
      tags: [],
      categories: [],
      relation_types: [],
    }),
  };
}

/** Every path the app asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => call[0]);
}

/** How many times a route was asked for, exactly. */
function reads(path: string): number {
  return requested().filter((asked) => asked === path).length;
}

/**
 * The top bar, which is the one place this action is drawn.
 *
 * Found through the search box rather than by the banner role: a screen behind
 * the frame carries a header of its own, and the proposals card carries a share
 * button of its own, so an unscoped query would answer with either.
 */
async function topBar(): Promise<HTMLElement> {
  const search = await screen.findByRole("search");
  const header = search.closest("header");
  if (header === null) {
    throw new Error("the search box is not in the top bar");
  }
  return header;
}

/** The action itself, once the frame has decided to offer it. */
async function shareAction(): Promise<HTMLElement> {
  return within(await topBar()).findByRole("button", { name: "Share changes" });
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the top bar's share action", () => {
  it("offers nothing to a session that may not administer", async () => {
    serve({ "/settings/github": () => githubStatus() }, "editor");

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    expect(
      within(await topBar()).queryByRole("button", { name: "Share changes" }),
    ).toBeNull();
    // And the two reads behind the action never happen: both are admin-only
    // routes, and the summary probes GitHub for every team domain at once.
    expect(requested()).not.toContain("/settings/github");
    expect(requested()).not.toContain("/sync");
  });

  it("offers nothing on an instance with GitHub switched off", async () => {
    serve({ "/settings/github": () => githubStatus({ enabled: false }) });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });
    await waitFor(() => {
      expect(requested()).toContain("/settings/github");
    });

    expect(
      within(await topBar()).queryByRole("button", { name: "Share changes" }),
    ).toBeNull();
    // Nothing to summarize when no origin can be reached at all.
    expect(requested()).not.toContain("/sync");
  });

  it("offers nothing on a read-only instance", async () => {
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () =>
          meResponse({ user: userFixture({ role: "admin" }), read_only: true }),
        "/domains": domainsResponse,
        "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
        "/settings/github": () => githubStatus(),
      }),
    );

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    // Sharing writes to the origin, and this instance refuses writes.
    expect(
      within(await topBar()).queryByRole("button", { name: "Share changes" }),
    ).toBeNull();
    expect(requested()).not.toContain("/sync");
  });

  it("says to connect GitHub before it says there is nothing to share", async () => {
    serve({
      "/settings/github": () => githubStatus({ connected: false, user: null }),
    });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    const action = await shareAction();
    expect(action).toHaveAttribute("aria-disabled", "true");
    // The reason is on the control rather than behind a dialog that would
    // refuse: the feature is on, so the button belongs on screen saying why
    // it will not act.
    await userEvent.hover(action);
    expect(
      await screen.findByRole("tooltip", {}, { timeout: 2000 }),
    ).toHaveTextContent("Connect GitHub first");
    // No credential means no origin to summarize either.
    expect(requested()).not.toContain("/sync");
  });

  it("shares the domain being read, without asking which one", async () => {
    serve({
      "/settings/github": () => githubStatus(),
      "/domains/eng/sync": () => syncResponse(),
      "/domains/eng/sync/changes": () => ({
        action: "create",
        effective_title: "Share 2 new engrams from eng",
        changes: [{ path: "notes/a.md", kind: "added" }],
      }),
      ...domainScreenRoutes("eng"),
    });

    renderApp("/d/eng");
    const action = await shareAction();
    await waitFor(() => {
      expect(action).toHaveAttribute("aria-disabled", "false");
    });

    await userEvent.click(action);

    const dialog = await screen.findByRole("dialog", { name: "Share changes" });
    expect(
      await within(dialog).findByText(/opens a new proposal/i),
    ).toBeInTheDocument();
    // The domain being read is the answer, so the instance-wide question is
    // never asked: one probe per share action rather than one per domain.
    expect(requested()).not.toContain("/sync");
  });

  it("says which domain has nothing to share, by name", async () => {
    serve({
      "/settings/github": () => githubStatus(),
      "/domains/eng/sync": () => syncResponse({ local_changes: 0 }),
      ...domainScreenRoutes("eng"),
    });

    renderApp("/d/eng");
    const action = await shareAction();
    await waitFor(() => {
      expect(action).toHaveAttribute("aria-disabled", "true");
    });

    await userEvent.hover(action);
    expect(
      await screen.findByRole("tooltip", {}, { timeout: 2000 }),
    ).toHaveTextContent("Nothing to share in eng");
    // And it does not act: an aria-disabled control is one a pointer can
    // reach, so the guard has to be on the press as well as on the face.
    await userEvent.click(action);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("asks which domain when the reader is not standing in one", async () => {
    serve({
      "/settings/github": () => githubStatus(),
      "/sync": () =>
        summaryResponse([summaryEntry("eng", 0), summaryEntry("ops", 2)]),
      "/domains/ops/sync/changes": () => ({
        action: "create",
        effective_title: "Share 2 new engrams from ops",
        changes: [{ path: "notes/a.md", kind: "added" }],
      }),
    });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });
    const action = await shareAction();
    await waitFor(() => {
      expect(action).toHaveAttribute("aria-disabled", "false");
    });

    await userEvent.click(action);

    // Only the domains with work waiting, each sized so the choice between
    // them is a choice rather than a guess.
    const picker = await screen.findByRole("dialog", {
      name: "Share from a domain",
    });
    expect(within(picker).queryByRole("button", { name: /^eng/ })).toBeNull();
    await userEvent.click(
      within(picker).getByRole("button", { name: "ops - 2 pending changes" }),
    );

    // The picker hands over to the same dialog the domain route opens.
    const dialog = await screen.findByRole("dialog", { name: "Share changes" });
    expect(
      await within(dialog).findByText(/opens a new proposal/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("dialog", { name: "Share from a domain" }),
    ).toBeNull();
  });

  it("stops reading the picked domain's status once the share has landed", async () => {
    const shared = vi.fn(() => ({
      outcome: "proposed",
      number: 7,
      url: "https://github.com/acme/ops/pull/7",
    }));
    serve({
      "/settings/github": () => githubStatus(),
      "/sync": () => summaryResponse([summaryEntry("ops", 2)]),
      // The dialog reads this for the open layers it offers to amend. Nothing
      // on this screen is drawn from it - the proposals card lives on the
      // domain screen - so this observer is the only one there is.
      "/domains/ops/sync": () => syncResponse({ domain: "ops" }),
      "/domains/ops/sync/changes": () => ({
        action: "create",
        effective_title: "Share 2 new engrams from ops",
        changes: [{ path: "notes/a.md", kind: "added" }],
      }),
      "/domains/ops/sync/share": (_path, init) =>
        init?.method === "POST" ? shared() : null,
    });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });
    const action = await shareAction();
    await waitFor(() => {
      expect(action).toHaveAttribute("aria-disabled", "false");
    });
    await userEvent.click(action);
    const picker = await screen.findByRole("dialog", {
      name: "Share from a domain",
    });
    await userEvent.click(
      within(picker).getByRole("button", { name: "ops - 2 pending changes" }),
    );

    const dialog = await screen.findByRole("dialog", { name: "Share changes" });
    await waitFor(() => {
      expect(
        within(dialog).getByRole("button", { name: "Share" }),
      ).toBeEnabled();
    });
    const statusReads = reads("/domains/ops/sync");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Share" }),
    );

    await within(dialog).findByText(/opened proposal #7/i);
    // The share's own success handler invalidates the status key along with
    // the two listings, and reading that key pulls the origin. Waiting for
    // the summary - invalidated in the same tick, and read by the action
    // behind this dialog - is what makes the count below a settled answer
    // rather than a race.
    await waitFor(() => {
      expect(reads("/sync")).toBeGreaterThan(1);
    });
    // The form is gone, the select with it, and nothing left on screen is
    // drawn from this domain's status: a second origin-pulling read here
    // would be a probe to redraw something nobody is looking at.
    expect(reads("/domains/ops/sync")).toBe(statusReads);
  });

  it("badges a domain whose chain is wedged before anybody picks it", async () => {
    serve({
      "/settings/github": () => githubStatus(),
      "/sync": () =>
        summaryResponse([
          summaryEntry("eng", 2, { stack_wedged: [3] }),
          summaryEntry("ops", 1, { repair_pending: true }),
        ]),
    });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });
    const action = await shareAction();
    await waitFor(() => {
      expect(action).toHaveAttribute("aria-disabled", "false");
    });
    await userEvent.click(action);

    const picker = await screen.findByRole("dialog", {
      name: "Share from a domain",
    });
    // A wedged chain cannot grow, so the row says so before somebody picks it
    // and finds out from a refusal.
    expect(within(picker).getByText("stack wedged")).toBeVisible();
    expect(within(picker).getByText("repair pending")).toBeVisible();
    // And the badge reaches a reader who hears the row rather than sees it.
    expect(
      within(picker).getByRole("button", {
        name: "eng - 2 pending changes, stack wedged",
      }),
    ).toBeVisible();
  });

  it("counts what is yours on the button, and says the pairing in words", async () => {
    serve({
      "/settings/github": () => githubStatus(),
      "/domains/eng/sync": () =>
        syncResponse({ local_changes: 5, owned_changes: 2 }),
      ...domainScreenRoutes("eng"),
    });

    renderApp("/d/eng");
    const action = await shareAction();
    await waitFor(() => {
      expect(action).toHaveAttribute("aria-disabled", "false");
    });

    // The badge is this reader's own work where there is any: it is what they
    // would be sharing, and the total is one hover away.
    await waitFor(() => {
      expect(action).toHaveTextContent("2");
    });
    await userEvent.hover(action);
    expect(
      await screen.findByRole("tooltip", {}, { timeout: 2000 }),
    ).toHaveTextContent("2 of 5 unshared changes are yours");
  });

  it("falls back to everything waiting when none of it is yours", async () => {
    serve({
      "/settings/github": () => githubStatus(),
      "/domains/eng/sync": () =>
        syncResponse({ local_changes: 5, owned_changes: 0 }),
      ...domainScreenRoutes("eng"),
    });

    renderApp("/d/eng");
    const action = await shareAction();
    await waitFor(() => {
      expect(action).toHaveAttribute("aria-disabled", "false");
    });

    // A `0` badge beside a live share action would read as nothing to share,
    // so the badge is how much is waiting - and the tooltip still says whose.
    await waitFor(() => {
      expect(action).toHaveTextContent("5");
    });
    await userEvent.hover(action);
    expect(
      await screen.findByRole("tooltip", {}, { timeout: 2000 }),
    ).toHaveTextContent("0 of 5 unshared changes are yours");
  });

  it("says only what it knows when the report names no owner", async () => {
    serve({
      "/settings/github": () => githubStatus(),
      // No owned count at all, which is every report an older server sends.
      "/domains/eng/sync": () => syncResponse({ local_changes: 3 }),
      ...domainScreenRoutes("eng"),
    });

    renderApp("/d/eng");
    const action = await shareAction();
    await waitFor(() => {
      expect(action).toHaveAttribute("aria-disabled", "false");
    });

    await waitFor(() => {
      expect(action).toHaveTextContent("3");
    });
    await userEvent.hover(action);
    const tip = await screen.findByRole("tooltip", {}, { timeout: 2000 });
    expect(tip).toHaveTextContent("Share changes");
    expect(tip).not.toHaveTextContent("yours");
  });

  it("pairs each picker row's own work with its total", async () => {
    serve({
      "/settings/github": () => githubStatus(),
      "/sync": () =>
        summaryResponse([
          summaryEntry("eng", 5, { owned_changes: 2 }),
          summaryEntry("ops", 3),
        ]),
    });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });
    const action = await shareAction();
    await waitFor(() => {
      expect(action).toHaveAttribute("aria-disabled", "false");
    });
    // Summed across the domains that answered: two of the eight waiting
    // changes are this reader's.
    await waitFor(() => {
      expect(action).toHaveTextContent("2");
    });

    await userEvent.click(action);
    const picker = await screen.findByRole("dialog", {
      name: "Share from a domain",
    });
    expect(within(picker).getByText("2 yours")).toBeVisible();
    // Spoken as the same pairing, and the row that carries no owned count
    // still reads exactly as it always did.
    expect(
      within(picker).getByRole("button", {
        name: "eng - 5 pending changes, 2 yours",
      }),
    ).toBeVisible();
    expect(
      within(picker).getByRole("button", { name: "ops - 3 pending changes" }),
    ).toBeVisible();
  });

  it("has nothing to offer when no domain anywhere is holding work", async () => {
    serve({
      "/settings/github": () => githubStatus(),
      "/sync": () => summaryResponse([summaryEntry("eng", 0)]),
    });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });
    const action = await shareAction();
    await waitFor(() => {
      expect(action).toHaveAttribute("aria-disabled", "true");
    });

    await userEvent.hover(action);
    expect(
      await screen.findByRole("tooltip", {}, { timeout: 2000 }),
    ).toHaveTextContent("Nothing to share");
  });

  it("falls back to the whole instance inside a domain with no origin", async () => {
    serve({
      "/settings/github": () => githubStatus(),
      // A local domain has no origin at all, which the route says with a 404.
      "/domains/eng/sync": () => {
        throw new ApiProblem(404, "not found", "eng has no origin");
      },
      "/sync": () => summaryResponse([summaryEntry("ops", 1)]),
      ...domainScreenRoutes("eng"),
    });

    renderApp("/d/eng");
    const action = await shareAction();
    await waitFor(() => {
      expect(action).toHaveAttribute("aria-disabled", "false");
    });

    // Standing in a domain that cannot be shared is not a dead end: the
    // question becomes the one the reader would have meant anyway.
    await userEvent.click(action);
    expect(
      await screen.findByRole("button", { name: "ops - 1 pending change" }),
    ).toBeVisible();
  });

  it("offers the same act in the palette, and only while it would work", async () => {
    serve({
      "/settings/github": () => githubStatus(),
      "/sync": () => summaryResponse([summaryEntry("ops", 2)]),
    });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });
    await waitFor(() => {
      expect(requested()).toContain("/sync");
    });

    const user = userEvent.setup();
    await user.keyboard("{Meta>}k{/Meta}");
    await user.click(
      await screen.findByRole("option", { name: "Share changes" }),
    );

    expect(
      await screen.findByRole("dialog", { name: "Share from a domain" }),
    ).toBeVisible();
  });

  it("asks each of its questions once, however many things read the answer", async () => {
    serve({
      "/settings/github": () => githubStatus(),
      "/sync": () => summaryResponse([summaryEntry("ops", 2)]),
    });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });
    const action = await shareAction();
    await waitFor(() => {
      expect(action).toHaveAttribute("aria-disabled", "false");
    });

    const user = userEvent.setup();
    await user.keyboard("{Meta>}k{/Meta}");
    // Both readers are live now: the button in the top bar, and the palette
    // row, which is registered by the frame off the same verdict.
    expect(
      await screen.findByRole("option", { name: "Share changes" }),
    ).toBeVisible();

    // Two observers of each query rather than two requests. This is the whole
    // reason the verdict may be worked out in two places at once, and the
    // reason it matters here rather than in general: reading either of these
    // reaches GitHub, the summary once per team domain.
    expect(reads("/settings/github")).toBe(1);
    expect(reads("/sync")).toBe(1);
  });

  it("registers no palette row for a share that would refuse", async () => {
    serve({
      "/settings/github": () => githubStatus(),
      "/sync": () => summaryResponse([summaryEntry("eng", 0)]),
    });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });
    await waitFor(() => {
      expect(requested()).toContain("/sync");
    });

    const user = userEvent.setup();
    await user.keyboard("{Meta>}k{/Meta}");
    await screen.findByRole("dialog");

    // The palette has no disabled state, so an act that would refuse is not
    // offered in it at all - the button carries the reason instead.
    expect(screen.queryByRole("option", { name: "Share changes" })).toBeNull();
  });
});
