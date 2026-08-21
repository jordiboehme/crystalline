/**
 * One domain: what it is for, what is in it, and the two states that must never
 * look alike - a domain nobody registered, and a registered domain with nothing
 * in it yet. The first is a wrong address and the second is an invitation, so
 * the screen says which one happened rather than showing one empty box for
 * both.
 *
 * The browse view and the filter view are the same paged endpoint asked two
 * different questions - one folder, or one set of frontmatter filters across
 * the whole domain - and the screen names which one is on screen instead of
 * blending them. The tree is what the folder navigation above the list is
 * drawn from, and nothing else.
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

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

const MANIFEST = [
  "---",
  "title: eng",
  "---",
  "",
  "# eng",
  "",
  "What this domain is for, in one paragraph.",
  "",
  "## When to Use",
  "",
  "- Route here for eng questions.",
  "",
].join("\n");

/** The tree, which answers with the folder that was asked for. */
function treeResponse(path: string) {
  if (path.includes("path=notes")) {
    return {
      domain: "eng",
      path: "notes",
      folders: [],
      engrams: [
        {
          permalink: "notes/beta",
          title: "Beta",
          type: "engram",
          path: "notes/beta.md",
        },
      ],
    };
  }
  return {
    domain: "eng",
    path: "/",
    folders: ["notes"],
    engrams: [
      { permalink: "alpha", title: "Alpha", type: "engram", path: "alpha.md" },
    ],
  };
}

/**
 * The listing, which is what both views of this screen page: the frontmatter
 * one across the whole domain, and the folder one scoped by `path`. The
 * filtered answer carries a retired engram, so the fade has something to do.
 */
function engramsResponse(path: string) {
  if (path.includes("tags=eng")) {
    return {
      mode: "text",
      total: 1,
      page: 1,
      limit: 50,
      count: 1,
      hits: [
        {
          domain: "eng",
          permalink: "gamma",
          title: "Gamma",
          engram_type: "decision",
          kind: "engram",
          status: "deprecated",
          tags: ["eng"],
          score: 1,
          snippet: "A decision that no longer holds.",
        },
      ],
    };
  }
  if (path.includes("path=notes")) {
    return {
      mode: "text",
      total: 1,
      page: 1,
      limit: 50,
      count: 1,
      hits: [
        {
          domain: "eng",
          permalink: "notes/beta",
          title: "Beta",
          engram_type: "engram",
          kind: "engram",
          status: "stable",
          tags: [],
        },
      ],
    };
  }
  return {
    mode: "text",
    total: 2,
    page: 1,
    limit: 50,
    count: 2,
    hits: [
      {
        domain: "eng",
        permalink: "alpha",
        title: "Alpha",
        engram_type: "engram",
        kind: "engram",
        status: "stable",
        tags: [],
      },
      {
        domain: "eng",
        permalink: "notes/beta",
        title: "Beta",
        engram_type: "engram",
        kind: "engram",
        status: "stable",
        tags: [],
      },
    ],
  };
}

/**
 * The sync status, in the count spelling: `open_proposals` as a number.
 *
 * The engine's per-domain report embeds the proposals themselves and its poll
 * overview counts them, so both spellings reach this screen. This is the short
 * one; the test below overrides `open_proposals` with the list the real
 * endpoint sends, so the card is pinned against both.
 */
function syncResponse(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    mode: "github",
    repo: "acme/kb",
    branch: "main",
    last_checked: "2026-08-10T08:00:00Z",
    local_changes: 2,
    open_proposals: 1,
    behind: false,
    probe_error: null,
    ...overrides,
  };
}

function vocabularyResponse() {
  return {
    domain: "eng",
    tags: [{ name: "eng", engrams: 3, observations: 5 }],
    categories: [],
    relation_types: [],
  };
}

function serve(
  routes: Record<string, Answer> = {},
  role: "admin" | "editor" = "editor",
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role }) }),
      "/domains": domainsResponse,
      "/domains/eng/manifest": () => ({ domain: "eng", markdown: MANIFEST }),
      "/domains/eng/tree": treeResponse,
      "/domains/eng/engrams": engramsResponse,
      "/vocabulary": vocabularyResponse,
      ...routes,
    }),
  );
}

/** The listing as it reads for a domain of the given kind. */
function listingOf(kind: string) {
  const listing = domainsResponse();
  return { ...listing, domains: [{ ...listing.domains[0], kind }] };
}

/** Every path the app asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => call[0]);
}

/**
 * The screen itself, without the frame around it.
 *
 * Inside a domain the sidebar draws the same folders and the same engrams as
 * navigation, so a query for one of them has to say which of the two it means.
 */
async function screenBody(): Promise<HTMLElement> {
  return screen.findByRole("main");
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the domain screen", () => {
  it("renders the manifest's lede and the engrams at the root of the domain", async () => {
    serve();

    renderApp("/d/eng");

    expect(await screen.findByRole("heading", { name: "eng" })).toBeVisible();
    // The manifest is one paragraph here and a link to the rest: what a
    // reader arriving in a domain needs is what it is for, then its engrams,
    // and a whole document in between put the list below the fold.
    expect(
      await screen.findByText("What this domain is for, in one paragraph."),
    ).toBeVisible();
    expect(
      screen.getByRole("link", { name: "Read the MANIFEST" }),
    ).toHaveAttribute("href", "/d/eng/manifest");
    expect(screen.queryByRole("heading", { name: "When to Use" })).toBeNull();
    const row = await within(await screenBody()).findByRole("link", {
      name: /Alpha/,
    });
    expect(row).toHaveAttribute("href", "/d/eng/e/alpha");
  });

  it("says a manifest with no prose at all is there without quoting nothing", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: "---\ntitle: eng\n---\n\n# eng\n",
      }),
    });

    renderApp("/d/eng");

    // Headings and frontmatter are not a lede. The link is still the way in:
    // an empty summary is not the same fact as a missing MANIFEST.
    expect(
      await screen.findByRole("link", { name: "Read the MANIFEST" }),
    ).toBeVisible();
    expect(screen.queryByText(/no MANIFEST yet/)).toBeNull();
  });

  it("launches a new engram from the heading of the list it lands in", async () => {
    serve();

    renderApp("/d/eng");
    const body = await screenBody();

    // The primary tier: it is the one thing a writer comes to a domain to do,
    // and the sidebar's launcher never co-renders with it.
    const launcher = await within(body).findByRole("button", {
      name: "New engram",
    });
    expect(launcher.className).toContain("bg-accent-700");
  });

  it("says a domain nobody registered is not here", async () => {
    serve({
      "/domains/ghost/tree": () => {
        throw new ApiProblem(
          404,
          "not found",
          "no domain 'ghost' is registered",
        );
      },
      "/domains/ghost/manifest": () => {
        throw new ApiProblem(
          404,
          "not found",
          "no domain 'ghost' is registered",
        );
      },
    });

    renderApp("/d/ghost");

    expect(
      await screen.findByRole("heading", { name: "Domain not found" }),
    ).toBeVisible();
    // Distinct from an empty domain: nothing here invites the reader to add
    // an engram to a domain that does not exist.
    expect(screen.queryByText(/no engrams yet/)).toBeNull();
  });

  it("says an empty domain is empty, and not that it is missing", async () => {
    serve({
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
    });

    renderApp("/d/eng");

    // Scoped to the screen: the sidebar says the same thing about the same
    // empty domain, which is the frame's own line rather than this one.
    expect(
      await within(await screenBody()).findByText(/no engrams yet/),
    ).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "Domain not found" }),
    ).toBeNull();
  });

  it("treats a missing manifest as a gap, not as a missing domain", async () => {
    serve({
      "/domains/eng/manifest": () => {
        throw new ApiProblem(404, "not found", "no MANIFEST in domain 'eng'");
      },
    });

    renderApp("/d/eng");

    expect(await screen.findByText(/no MANIFEST yet/)).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "Domain not found" }),
    ).toBeNull();
    // The engrams are still listed: the manifest is one panel, not the screen.
    expect(
      await within(await screenBody()).findByRole("link", { name: /Alpha/ }),
    ).toBeVisible();
  });

  it("opens a folder into its own list", async () => {
    serve();

    renderApp("/d/eng");
    const body = await screenBody();
    await within(body).findByRole("link", { name: /Alpha/ });

    await userEvent.click(within(body).getByRole("button", { name: "notes" }));

    expect(
      await within(body).findByRole("link", { name: /Beta/ }),
    ).toHaveAttribute("href", "/d/eng/e/notes/beta");
    await waitFor(() => {
      expect(requested().some((path) => path.includes("path=notes"))).toBe(
        true,
      );
    });
  });

  it("pages a folder from the listing rather than from the tree", async () => {
    serve({
      "/domains/eng/engrams": (path) =>
        path.includes("path=notes")
          ? {
              mode: "text",
              total: 620,
              page: 1,
              limit: 50,
              count: 1,
              hits: [
                {
                  domain: "eng",
                  permalink: "notes/beta",
                  title: "Beta",
                  engram_type: "engram",
                  kind: "engram",
                  status: "stable",
                  tags: [],
                },
              ],
            }
          : engramsResponse(path),
    });

    // Straight to the folder, because the whole of this screen's state is its
    // URL: the same link somebody sends, and the same address the back button
    // returns to.
    renderApp("/d/eng?path=notes");
    const body = await screenBody();

    expect(
      await within(body).findByRole("link", { name: /Beta/ }),
    ).toBeVisible();
    // The listing endpoint, scoped and paged, rather than the tree's own rows:
    // a folder of six hundred engrams costs one page here.
    await waitFor(() => {
      expect(
        requested().some(
          (path) =>
            path.startsWith("/domains/eng/engrams?") &&
            path.includes("path=notes") &&
            path.includes("limit=50"),
        ),
      ).toBe(true);
    });
    // And the count is the envelope's, not the number of rows in hand.
    expect(
      await within(body).findByText(/620 engrams in this folder/),
    ).toBeVisible();
  });

  it("keeps a filter across the whole domain while a folder is open", async () => {
    serve();

    renderApp("/d/eng?path=notes");
    const body = await screenBody();
    await within(body).findByRole("link", { name: /Beta/ });

    await userEvent.click(within(body).getByRole("button", { name: /#eng/ }));

    await screen.findByRole("link", { name: /Gamma/ });
    // The filtered view is the whole domain, every folder included, and it
    // says so. Scoping it to the folder being browsed would be a different
    // feature, not a side effect of paging the browse view.
    expect(screen.getByText(/whole domain/i)).toBeVisible();
    const filtered = requested().filter(
      (path) =>
        path.startsWith("/domains/eng/engrams?") && path.includes("tags=eng"),
    );
    expect(filtered.length).toBeGreaterThan(0);
    expect(filtered.every((path) => !path.includes("path="))).toBe(true);
  });

  it("switches to the whole domain when a tag is filtered on", async () => {
    serve();

    renderApp("/d/eng");
    const body = await screenBody();
    await within(body).findByRole("link", { name: /Alpha/ });

    await userEvent.click(within(body).getByRole("button", { name: /#eng/ }));

    // The frontmatter view answers, and it is the one that carries status.
    const row = await screen.findByRole("link", { name: /Gamma/ });
    expect(row).toHaveTextContent("deprecated");
    await waitFor(() => {
      expect(
        requested().some(
          (path) =>
            path.startsWith("/domains/eng/engrams") &&
            path.includes("tags=eng"),
        ),
      ).toBe(true);
    });
    // And the screen says which view is on screen, so the switch is not a
    // silent one.
    expect(screen.getByText(/whole domain/i)).toBeVisible();
  });

  it("unregisters behind a second press, and says the files stay", async () => {
    const removed = vi.fn(() => ({ files_kept: true, rooms_closed: 0 }));
    serve(
      {
        "/domains/eng": (_path, init) =>
          init?.method === "DELETE" ? removed() : domainsResponse(),
        "/activity": () => ({ timeframe: "7d", items: [] }),
      },
      "admin",
    );

    renderApp("/d/eng");
    const body = await screenBody();

    await userEvent.click(
      await within(body).findByRole("button", { name: "Unregister domain" }),
    );
    // The first press only asks. Nothing has been unregistered yet.
    expect(removed).not.toHaveBeenCalled();
    expect(within(body).getByText(/files stay on disk/i)).toBeVisible();

    await userEvent.click(
      within(body).getByRole("button", { name: "Confirm unregister" }),
    );

    await waitFor(() => {
      expect(removed).toHaveBeenCalled();
    });
    // The domain the reader was on is gone, so the screen is: home, with the
    // listing every screen reads asked again rather than left one domain long.
    expect(
      await screen.findByRole("heading", { level: 1, name: "Home" }),
    ).toBeVisible();
    await waitFor(() => {
      expect(
        requested().filter((path) => path === "/domains").length,
      ).toBeGreaterThan(1);
    });
  });

  it("does not offer unregistering below admin", async () => {
    serve();

    renderApp("/d/eng");
    const body = await screenBody();
    await within(body).findByRole("link", { name: /Alpha/ });

    expect(
      within(body).queryByRole("button", { name: "Unregister domain" }),
    ).toBeNull();
  });

  it("offers the archive round trip to an admin", async () => {
    serve({}, "admin");

    renderApp("/d/eng");
    const body = await screenBody();

    // A link the browser saves rather than a fetch the app holds in memory:
    // the download is a cookie-authenticated GET, so the anchor is the whole
    // mechanism, and `download` is what makes it a save rather than a
    // navigation into a zip.
    const download = await within(body).findByRole("link", {
      name: "Download archive",
    });
    expect(download).toHaveAttribute("href", "/api/v1/domains/eng/archive");
    expect(download).toHaveAttribute("download");
    expect(
      within(body).getByRole("button", { name: "Import archive" }),
    ).toBeVisible();
  });

  it("offers neither half of the archive round trip below admin", async () => {
    serve();

    renderApp("/d/eng");
    const body = await screenBody();
    await within(body).findByRole("link", { name: /Alpha/ });

    // Both endpoints are admin-only, so neither control is drawn for anybody
    // who would be refused at it.
    expect(
      within(body).queryByRole("link", { name: "Download archive" }),
    ).toBeNull();
    expect(
      within(body).queryByRole("button", { name: "Import archive" }),
    ).toBeNull();
  });

  it("returns focus to the trigger when a refusal blocks the confirm", async () => {
    serve(
      {
        "/domains/eng": (_path, init) => {
          if (init?.method === "DELETE") {
            throw new ApiProblem(
              409,
              "conflict",
              "domain 'eng' is defined by the environment and cannot be unregistered here",
            );
          }
          return domainsResponse();
        },
      },
      "admin",
    );

    renderApp("/d/eng");
    const body = await screenBody();
    const trigger = await within(body).findByRole("button", {
      name: "Unregister domain",
    });

    await userEvent.click(trigger);
    await userEvent.click(
      within(body).getByRole("button", { name: "Confirm unregister" }),
    );

    // The refusal lives in the parent's mutation `onError`, which cannot
    // reach the child's trigger ref; the fix is a transition-aware effect in
    // the child, not a copy of `abandon()`'s one-liner.
    const alert = await within(body).findByRole("alert");
    expect(alert).toHaveTextContent(/cannot be unregistered/);
    // The confirm buttons unmounted with the refusal, which would otherwise
    // drop focus to the document body - a keyboard or screen-reader user
    // loses their place entirely. Identity, not merely "not the trigger":
    // asserting `document.activeElement` really is `trigger`.
    expect(document.activeElement).toBe(trigger);
    // `role="alert"` is already an implicit ARIA live region (assertive), so
    // the refusal text is announced without the trigger needing to point at
    // it: the connection decision 26 asks for already exists here.
    expect(alert).toHaveAttribute("role", "alert");
  });

  it("leaves focus where the blur path put it, not on the trigger", async () => {
    serve({}, "admin");

    renderApp("/d/eng");
    const body = await screenBody();
    const trigger = await within(body).findByRole("button", {
      name: "Unregister domain",
    });

    await userEvent.click(trigger);
    within(body).getByRole("button", { name: "Confirm unregister" });

    // Shift-tab out of the confirm row entirely: the first hop stays inside
    // it (trigger to confirm are siblings under the same wrapper), the
    // second leaves it for "Import archive", the control immediately before
    // it in the row. That crossing is what the wrapper's own `onBlur`
    // collapses `confirming` for - deliberately, because focus moved
    // somewhere else on purpose.
    await userEvent.tab({ shift: true });
    await userEvent.tab({ shift: true });
    const importButton = within(body).getByRole("button", {
      name: "Import archive",
    });

    // The counterweight: a fix that steals focus back to the trigger
    // whenever `confirming` goes false would pass a check that only asserts
    // "not the trigger" on a jsdom that parks focus on the body mid-blur.
    // Asserting identity against the actual destination is what catches
    // that over-reach.
    expect(document.activeElement).toBe(importButton);
    expect(
      within(body).queryByRole("button", { name: "Confirm unregister" }),
    ).toBeNull();
  });

  it("warns that a virtual domain's engrams go with it", async () => {
    serve(
      {
        "/domains": () => listingOf("virtual"),
        "/activity": () => ({ timeframe: "7d", items: [] }),
      },
      "admin",
    );

    renderApp("/d/eng");
    const body = await screenBody();

    await userEvent.click(
      await within(body).findByRole("button", { name: "Unregister domain" }),
    );

    // Nothing stays on disk here, so nothing here says it does: the engrams
    // are the database's, and the way to keep a copy is named.
    expect(within(body).getByText(/live in the database/i)).toBeVisible();
    expect(within(body).getByText(/download the archive first/i)).toBeVisible();
    expect(within(body).queryByText(/files stay on disk/i)).toBeNull();
  });
});

describe("the team sync card", () => {
  it("shows the sync card for an admin on a team domain", async () => {
    serve({ "/domains/eng/sync": () => syncResponse() }, "admin");

    renderApp("/d/eng");
    const body = await screenBody();

    const card = await within(body).findByRole("region", {
      name: "Team sync",
    });
    expect(within(card).getByText("acme/kb")).toBeVisible();
    expect(within(card).getByText("main")).toBeVisible();
    // The day the instant names, cut out of the string: this app never turns
    // a written date into a browser's local one.
    expect(within(card).getByText("2026-08-10")).toBeVisible();
    expect(within(card).getByText("2 pending local changes")).toBeVisible();
    expect(within(card).getByText("1 open proposal")).toBeVisible();
    // Nothing was declined and nothing conflicts, so neither is mentioned: a
    // zero of an exceptional thing is noise on a card that is otherwise fine.
    expect(within(card).queryByText(/declined proposal/)).toBeNull();
    expect(within(card).queryByText(/to settle/)).toBeNull();
    // Nothing failed, so nothing is announced as failed.
    expect(within(card).queryByRole("alert")).toBeNull();
  });

  it("names the declined proposals and the conflicts when there are any", async () => {
    serve(
      {
        "/domains/eng/sync": () =>
          syncResponse({
            declined_proposals: [{ number: 3 }, { number: 4 }],
            conflicts: ["notes/a.md"],
          }),
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await within(await screenBody()).findByRole("region", {
      name: "Team sync",
    });

    // Declined work is informational; a conflict is something somebody has to
    // go and do, so the wording says so.
    expect(within(card).getByText("2 declined proposals")).toBeVisible();
    expect(within(card).getByText("1 conflict to settle")).toBeVisible();
  });

  it("names the declined proposals without inventing a conflict row", async () => {
    // The two exceptional counts are two independent rows, and the test above
    // shows them together, which cannot tell a pair of rows apart from one row
    // that recites both counts. Each half on its own is what pins that: no
    // connection block in the fixture, so the not-connected hint is not on the
    // card either and the row assertions are about the counts alone.
    serve(
      {
        "/domains/eng/sync": () =>
          syncResponse({ declined_proposals: 2, conflicts: 0 }),
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await within(await screenBody()).findByRole("region", {
      name: "Team sync",
    });

    expect(within(card).getByText("2 declined proposals")).toBeVisible();
    expect(within(card).queryByText(/to settle/)).toBeNull();
  });

  it("names the conflicts without inventing a declined row", async () => {
    serve(
      {
        "/domains/eng/sync": () =>
          syncResponse({ declined_proposals: 0, conflicts: 2 }),
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await within(await screenBody()).findByRole("region", {
      name: "Team sync",
    });

    expect(within(card).getByText("2 conflicts to settle")).toBeVisible();
    expect(within(card).queryByText(/declined proposal/)).toBeNull();
  });

  it("counts the proposals the real endpoint actually sends", async () => {
    // The wire spelling: `origin_status`'s report embeds the open proposals
    // themselves rather than a count, so the card has to read a list here and
    // a number in the fixture above without knowing which it will get.
    serve(
      {
        "/domains/eng/sync": () =>
          syncResponse({
            local_changes: 1,
            open_proposals: [
              { number: 7, status: "open", url: null, title: "Add a runbook" },
              { number: 9, status: "open", url: null, title: "Fix the lede" },
            ],
            declined_proposals: [],
            conflicts: [],
            behind: true,
          }),
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await within(await screenBody()).findByRole("region", {
      name: "Team sync",
    });

    expect(within(card).getByText("2 open proposals")).toBeVisible();
    expect(within(card).getByText("1 pending local change")).toBeVisible();
    // The line that only exists when the origin is actually ahead.
    expect(within(card).getByText(/behind upstream/i)).toBeVisible();
  });

  it("says the numbers are stale when the origin check itself failed", async () => {
    serve(
      {
        "/domains/eng/sync": () =>
          syncResponse({
            last_checked: "2026-08-09T08:00:00Z",
            probe_error: "offline: could not reach api.github.com",
          }),
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await within(await screenBody()).findByRole("region", {
      name: "Team sync",
    });

    // The numbers still show - they are the local half of the report and they
    // are true about this copy - but nothing here lets them read as fresh.
    expect(within(card).getByText("2 pending local changes")).toBeVisible();
    const warning = within(card).getByRole("alert");
    expect(warning).toHaveTextContent(
      "offline: could not reach api.github.com",
    );
    expect(within(card).getByText(/2026-08-09 \(stale\)/)).toBeVisible();
  });

  it("says the instance is not connected, and where that is fixed", async () => {
    // The status route reports a missing connection rather than refusing over
    // it, so the card is the only place that answer is ever seen. Without this
    // row a disconnected instance shows a stale report and a probe error that
    // never names the actual cause.
    serve(
      {
        "/domains/eng/sync": () =>
          syncResponse({
            connection: { connected: false },
            probe_error: "no GitHub connection on this instance",
          }),
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await within(await screenBody()).findByRole("region", {
      name: "Team sync",
    });

    expect(
      await within(card).findByText(
        /not connected - connect GitHub under Settings to sync/i,
      ),
    ).toBeVisible();
  });

  it("says nothing about the connection when there is one, or no answer", async () => {
    serve(
      {
        "/domains/eng/sync": () =>
          syncResponse({ connection: { connected: true, user: "octo" } }),
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await within(await screenBody()).findByRole("region", {
      name: "Team sync",
    });

    expect(within(card).getByText("acme/kb")).toBeVisible();
    expect(within(card).queryByText(/not connected/i)).toBeNull();
  });

  it("says nothing about the connection when the report carries none", async () => {
    // A report with no connection block at all is not a report of a missing
    // connection: an older server, or one that dropped the key, must not make
    // this card tell somebody to go and connect what is already connected.
    serve({ "/domains/eng/sync": () => syncResponse() }, "admin");

    renderApp("/d/eng");
    const card = await within(await screenBody()).findByRole("region", {
      name: "Team sync",
    });

    expect(within(card).getByText("acme/kb")).toBeVisible();
    expect(within(card).queryByText(/not connected/i)).toBeNull();
  });

  it("pulls the origin and refreshes what the pull changed", async () => {
    const pulled = vi.fn(() => ({
      domain: "eng",
      up_to_date: false,
      applied: ["notes/a.md"],
    }));
    serve(
      {
        "/domains/eng/sync": (_path, init) =>
          init?.method === "POST" ? pulled() : syncResponse(),
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await within(await screenBody()).findByRole("region", {
      name: "Team sync",
    });
    const before = requested().filter((path) => path === "/domains").length;

    await userEvent.click(
      within(card).getByRole("button", { name: "Sync now" }),
    );

    await waitFor(() => {
      expect(pulled).toHaveBeenCalled();
    });
    // Both of the things a pull can have changed are asked again: this card's
    // own status, and the listing every sidebar and card draws from.
    await waitFor(() => {
      expect(
        requested().filter((path) => path === "/domains/eng/sync").length,
      ).toBeGreaterThan(1);
      expect(
        requested().filter((path) => path === "/domains").length,
      ).toBeGreaterThan(before);
    });
  });

  it("shows no card on a domain with no origin", async () => {
    serve(
      {
        "/domains/eng/sync": () => {
          throw new ApiProblem(
            404,
            "not found",
            "domain 'eng' has no team origin",
          );
        },
      },
      "admin",
    );

    renderApp("/d/eng");
    const body = await screenBody();
    await within(body).findByRole("link", { name: /Alpha/ });

    // A local domain has no sync status, which is not a failure to report.
    expect(
      within(body).queryByRole("region", { name: "Team sync" }),
    ).toBeNull();
    expect(within(body).queryByText(/no team origin/)).toBeNull();
  });

  it("keeps a non-404 refusal inside the card", async () => {
    serve(
      {
        "/domains/eng/sync": () => {
          throw new ApiProblem(
            409,
            "conflict",
            "GitHub is disabled on this instance: connect it under Settings > GitHub",
          );
        },
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await within(await screenBody()).findByRole("region", {
      name: "Team sync",
    });

    expect(within(card).getByRole("alert")).toHaveTextContent(
      "GitHub is disabled on this instance",
    );
    // No numbers stand in for the ones the server refused to give.
    expect(within(card).queryByText("acme/kb")).toBeNull();
    expect(within(card).queryByText(/pending local change/)).toBeNull();
    // The button stays: pressing it re-surfaces the same refusal in place.
    expect(
      within(card).getByRole("button", { name: "Sync now" }),
    ).toBeVisible();
  });

  it("keeps the numbers it already showed when a later check is refused", async () => {
    let reads = 0;
    serve(
      {
        "/domains/eng/sync": (_path, init) => {
          if (init?.method === "POST") {
            return { domain: "eng", up_to_date: true, applied: [] };
          }
          reads += 1;
          if (reads > 1) {
            throw new ApiProblem(
              409,
              "conflict",
              "GitHub is disabled on this instance",
            );
          }
          return syncResponse();
        },
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await within(await screenBody()).findByRole("region", {
      name: "Team sync",
    });
    expect(within(card).getByText("acme/kb")).toBeVisible();

    await userEvent.click(
      within(card).getByRole("button", { name: "Sync now" }),
    );

    // The refusal is announced, and what was already on screen stays there: a
    // refetch that failed is a card that could not be updated, not a card
    // whose facts were withdrawn.
    await waitFor(() => {
      expect(within(card).getByRole("alert")).toHaveTextContent(
        "GitHub is disabled on this instance",
      );
    });
    expect(within(card).getByText("acme/kb")).toBeVisible();
    expect(within(card).getByText("2 pending local changes")).toBeVisible();
  });

  it("says a domain that was never checked was never checked", async () => {
    serve(
      { "/domains/eng/sync": () => syncResponse({ last_checked: null }) },
      "admin",
    );

    renderApp("/d/eng");
    const card = await within(await screenBody()).findByRole("region", {
      name: "Team sync",
    });

    // A day that does not exist gets no staleness marker and no invented
    // date: a registered team domain the poller has not reached yet.
    expect(within(card).getByText("not yet")).toBeVisible();
    expect(within(card).queryByText(/stale/)).toBeNull();
  });

  it("does not say the same refusal twice", async () => {
    const refusal = "GitHub is disabled on this instance";
    serve(
      {
        "/domains/eng/sync": () => {
          throw new ApiProblem(409, "conflict", refusal);
        },
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await within(await screenBody()).findByRole("region", {
      name: "Team sync",
    });

    await userEvent.click(
      within(card).getByRole("button", { name: "Sync now" }),
    );

    // The pull is refused for the reason the status already gives, so the
    // card says it once: two byte-identical alerts read as two problems.
    await waitFor(() => {
      expect(within(card).getAllByRole("alert")).toHaveLength(1);
    });
    expect(within(card).getByRole("alert")).toHaveTextContent(refusal);
  });

  it("asks for no sync status below admin", async () => {
    serve({ "/domains/eng/sync": () => syncResponse() });

    renderApp("/d/eng");
    const body = await screenBody();
    await within(body).findByRole("link", { name: /Alpha/ });

    // The endpoints are admin-only, so an editor's screen must not knock on
    // them at all: a 403 in the console is noise nobody can act on.
    expect(
      requested().some((path) => path.startsWith("/domains/eng/sync")),
    ).toBe(false);
    expect(
      within(body).queryByRole("region", { name: "Team sync" }),
    ).toBeNull();
  });
});
