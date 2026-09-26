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

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import App from "../App";
import { ApiProblem, api } from "../api/client";
import { defined } from "../test/assert";
import type { Answer } from "../test/harness";
import {
  answersFor,
  domainsResponse,
  manifestSectionsResponse,
  meResponse,
  policyRow,
  renderApp,
  userFixture,
} from "../test/harness";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

/**
 * The graph renderer paints to a canvas, which jsdom has none of. Only the
 * old-address redirect suite below lands on the engram screen, which draws
 * this; every other test in this file never mounts it.
 */
vi.mock("../components/GraphCanvas", () => ({
  default: () => <div data-testid="canvas" />,
}));

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

/**
 * The sections a server reads out of `MANIFEST`, in the wire shape. The
 * fixture itself is the harness's, shared with the policies card's own suite.
 */
const sectionsResponse = manifestSectionsResponse;

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
  probe: () => unknown = () => meResponse({ user: userFixture({ role }) }),
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": probe,
      "/domains": domainsResponse,
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: MANIFEST,
        sections: sectionsResponse(),
      }),
      "/domains/eng/tree": treeResponse,
      "/domains/eng/engrams": engramsResponse,
      "/vocabulary": vocabularyResponse,
      // `MembersCard` reads this unconditionally, the way the tree and the
      // engram listing are read unconditionally: a shared domain, which
      // `eng` is unless a test says otherwise, so the card draws its plain
      // "shared" state and nothing else on this screen changes shape.
      "/domains/eng/members": () => ({
        owner: null,
        visibility: "shared",
        members: [],
      }),
      ...routes,
    }),
  );
}

/**
 * A probe from a server that predates `can_share`, so this side has to fall
 * back to the rule that held before the field existed.
 */
function olderProbe(role: "admin" | "editor"): () => unknown {
  return () => {
    const probe: Record<string, unknown> = {
      ...meResponse({ user: userFixture({ role }) }),
    };
    delete probe.can_share;
    return probe;
  };
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
  // The listing's order is remembered across sessions, so each test starts
  // from a browser that has never been told anything.
  localStorage.clear();
});

describe("the domain screen", () => {
  it("renders the whole MANIFEST and the engrams at the root of the domain", async () => {
    serve();

    renderApp("/d/eng");

    expect(
      await screen.findByRole("heading", { level: 1, name: "eng" }),
    ).toBeVisible();
    // The whole document, rendered where the domain is introduced: its
    // prose, its sections, the lot. The MANIFEST's own `# eng` folds into
    // the page's heading, so the domain is named once.
    expect(
      await screen.findByText("What this domain is for, in one paragraph."),
    ).toBeVisible();
    expect(
      screen.getByRole("heading", { level: 2, name: "When to Use" }),
    ).toBeVisible();
    expect(screen.getAllByRole("heading", { name: "eng" })).toHaveLength(1);
    expect(
      screen.queryByRole("link", { name: "Read the MANIFEST" }),
    ).toBeNull();
    const row = await within(await screenBody()).findByRole("link", {
      name: /Alpha/,
    });
    expect(row).toHaveAttribute("href", "/d/eng/e/alpha");
    // The count and the order menu sit together on the row above the list,
    // not under the heading, and the old caption is gone.
    const body = await screenBody();
    const count = within(body).getByText("2 engrams in this domain");
    expect(count).toBeVisible();
    expect(count.parentElement).not.toBeNull();
    expect(
      within(count.parentElement as HTMLElement).getByRole("button", {
        name: "Order: Newest first",
      }),
    ).toBeVisible();
    expect(
      within(body).queryByText(/, by the date they were recorded\.$/),
    ).toBeNull();
    await waitFor(() => {
      expect(
        requested().some(
          (path) =>
            path.startsWith("/domains/eng/engrams?") &&
            path.includes("sort=recorded") &&
            path.includes("dir=desc"),
        ),
      ).toBe(true);
    });
  });

  it("offers Edit MANIFEST to an admin", async () => {
    serve({}, "admin");

    renderApp("/d/eng");

    const section = await screen.findByRole("region", { name: "Manifest" });
    expect(
      await within(section).findByRole("link", { name: "Edit MANIFEST" }),
    ).toHaveAttribute("href", "/d/eng/manifest/edit");
  });

  it("offers no Edit MANIFEST below admin", async () => {
    serve();

    renderApp("/d/eng");

    const section = await screen.findByRole("region", { name: "Manifest" });
    await within(section).findByText(
      "What this domain is for, in one paragraph.",
    );
    expect(
      within(section).queryByRole("link", { name: "Edit MANIFEST" }),
    ).toBeNull();
  });
  it("shows the manifest's features when every section is there", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: MANIFEST,
        sections: sectionsResponse({
          scope: ["Everything about eng"],
          missing: [],
          provisioning: {
            decls: [
              { kind: "skills", path: "skills" },
              { kind: "agents", path: "../agents" },
            ],
            problems: [],
          },
          tag_aliases: {
            decls: [{ alias: "Multi_Word", canonical: "multi-word" }],
            problems: [],
          },
          policies: [
            policyRow(
              "generated_indexes",
              "shared",
              "shared",
              ["local", "shared"],
              "local",
              "Whether the generated folder listings travel with a share.",
            ),
            policyRow(
              "sharing",
              null,
              "proposal",
              ["proposal", "direct"],
              "proposal",
              "Whether a share opens a proposal for review or commits straight to the branch.",
            ),
          ],
        }),
      }),
    });

    renderApp("/d/eng");

    const routing = await screen.findByRole("region", { name: "Routing" });
    expect(
      within(routing).getByRole("heading", { name: "When to Use" }),
    ).toBeVisible();
    expect(
      within(routing).getByText("Route here for eng questions."),
    ).toBeVisible();
    expect(
      within(routing).getByRole("heading", { name: "Scope" }),
    ).toBeVisible();
    expect(within(routing).getByText("Everything about eng")).toBeVisible();
    expect(
      within(routing).getByText("Agents route by When to Use."),
    ).toBeVisible();

    const provisioning = screen.getByRole("region", { name: "Provisioning" });
    expect(within(provisioning).getByText("skills: skills")).toBeVisible();
    expect(within(provisioning).getByText("agents: ../agents")).toBeVisible();
    expect(within(provisioning).queryByText("Nothing declared")).toBeNull();

    const aliases = screen.getByRole("region", { name: "Tag aliases" });
    expect(within(aliases).getByText("Multi_Word -> multi-word")).toBeVisible();
    expect(within(aliases).queryByText("No aliases")).toBeNull();

    // The switches are their own card now, below the manifest rather than a
    // fifth panel inside it, and every registry key has a row there.
    const policies = screen.getByRole("region", { name: "Domain policies" });
    const generated = within(policies).getByRole("row", {
      name: /^generated_indexes/,
    });
    // The declared cell, which is the first: what holds reads `shared` too,
    // so the row is asked by position rather than by the word.
    expect(
      defined(within(generated).getAllByRole("cell")[0], "the declared cell"),
    ).toHaveTextContent("shared");
    expect(
      within(policies).getByRole("row", { name: /^sharing/ }),
    ).toBeVisible();
  });

  it("draws a routing bullet's markdown instead of its punctuation", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: MANIFEST,
        sections: sectionsResponse({
          when_to_use: ["**Search `project2030` first**"],
        }),
      }),
    });

    renderApp("/d/eng");

    const routing = await screen.findByRole("region", { name: "Routing" });
    const code = await within(routing).findByText("project2030");
    expect(code.tagName).toBe("CODE");
    expect(code.closest("strong")).not.toBeNull();
    expect(within(routing).queryByText(/\*\*/)).toBeNull();
  });

  it("names what a MANIFEST lacks, panel by panel", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: MANIFEST,
        sections: sectionsResponse({
          scope: ["Everything about eng"],
          when_to_use: [],
          routing: "scope",
          missing: ["When to Use"],
        }),
      }),
    });

    renderApp("/d/eng");

    // What an undeclared section says is what it is FOR, in the server's own
    // words, and the syntax that starts it: naming the absence alone taught a
    // reader nothing they did not already know from the empty box.
    const routing = await screen.findByRole("region", { name: "Routing" });
    expect(
      within(routing).getByText(/Agents pick this domain by these bullets/),
    ).toBeVisible();
    expect(
      within(routing).queryByText(
        /What belongs in this domain and what does not/,
      ),
    ).toBeNull();
    expect(
      within(routing).getByText(
        "Agents route by Scope, because When to Use is absent or empty.",
      ),
    ).toBeVisible();
    expect(
      within(screen.getByRole("region", { name: "Provisioning" })).getByText(
        /Folders this domain installs into an AI harness/,
      ),
    ).toBeVisible();
    expect(
      within(screen.getByRole("region", { name: "Tag aliases" })).getByText(
        /Spellings that fold into one canonical tag/,
      ),
    ).toBeVisible();
    const policies = screen.getByRole("region", { name: "Domain policies" });
    const generated = within(policies).getByRole("row", {
      name: /^generated_indexes/,
    });
    expect(within(generated).getByText("not declared")).toBeVisible();
  });

  it("says when no agent can route here", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: MANIFEST,
        sections: sectionsResponse({
          when_to_use: [],
          routing: "none",
          missing: ["Scope", "When to Use"],
        }),
      }),
    });

    renderApp("/d/eng");

    const routing = await screen.findByRole("region", { name: "Routing" });
    expect(
      within(routing).getByText(
        /What belongs in this domain and what does not/,
      ),
    ).toBeVisible();
    expect(
      within(routing).getByText(/Agents pick this domain by these bullets/),
    ).toBeVisible();
    expect(
      within(routing).getByText(
        "Agents cannot route here until When to Use has a bullet.",
      ),
    ).toBeVisible();
  });

  it("lists every problem bullet with its reason", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: MANIFEST,
        sections: sectionsResponse({
          provisioning: {
            decls: [{ kind: "skills", path: "skills" }],
            problems: [
              {
                kind: "unknown_type",
                bullet: "widgets: w",
                reason:
                  "unknown provisioning type `widgets`, expected one of skills, commands, agents or mcps",
              },
            ],
          },
          tag_aliases: {
            decls: [{ alias: "foo", canonical: "bar" }],
            problems: [
              {
                kind: "chained_alias",
                bullet: "foo -> bar",
                reason:
                  "canonical `bar` is itself an alias, resolution stays a single hop",
              },
            ],
          },
        }),
      }),
    });

    renderApp("/d/eng");

    const provisioning = await screen.findByRole("region", {
      name: "Provisioning",
    });
    expect(within(provisioning).getByText("widgets: w")).toBeVisible();
    expect(
      within(provisioning).getByText(
        "unknown provisioning type `widgets`, expected one of skills, commands, agents or mcps",
      ),
    ).toBeVisible();
    const aliases = screen.getByRole("region", { name: "Tag aliases" });
    // A chained alias is a declaration AND a problem, so the bullet stands
    // twice in the panel: once in the list the MANIFEST declares, once above
    // the reason it was flagged for.
    expect(within(aliases).getAllByText("foo -> bar")).toHaveLength(2);
    expect(
      within(aliases).getByText(
        "canonical `bar` is itself an alias, resolution stays a single hop",
      ),
    ).toBeVisible();
  });

  it("draws no panels for a server that sends no sections", async () => {
    serve({
      "/domains/eng/manifest": () => ({ domain: "eng", markdown: MANIFEST }),
    });

    renderApp("/d/eng");

    // An older daemon answers the markdown alone. The document is still
    // read; the panels, which would say nothing true, are not drawn.
    expect(
      await screen.findByText("What this domain is for, in one paragraph."),
    ).toBeVisible();
    expect(screen.queryByRole("region", { name: "Routing" })).toBeNull();
    // No sections, no card.
    expect(
      screen.queryByRole("region", { name: "Domain policies" }),
    ).toBeNull();
  });

  it("draws no policies card for a MANIFEST the server could not parse", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: MANIFEST,
        sections: sectionsResponse({ policies: [] }),
      }),
    });

    renderApp("/d/eng");

    // A document nobody can read declares nothing, so the server answers an
    // empty registry rather than one at its defaults: there is no switch to
    // offer over a MANIFEST that would have to be repaired first.
    expect(
      await screen.findByRole("region", { name: "Routing" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("region", { name: "Domain policies" }),
    ).toBeNull();
  });

  it("explains a section that is not declared, and offers to start it", async () => {
    serve({}, "admin");

    renderApp("/d/eng");

    const aliases = await screen.findByRole("region", { name: "Tag aliases" });
    // The meaning and the syntax, both the server's: a reader learns what the
    // section is for and what one line of it looks like without leaving the
    // page or already knowing.
    expect(
      within(aliases).getByText(/Spellings that fold into one canonical tag/),
    ).toBeVisible();
    expect(within(aliases).getByText(/k8s -> kubernetes/)).toBeVisible();
    expect(
      within(aliases).getByRole("button", { name: "Add Tag Aliases" }),
    ).toBeVisible();
  });

  it("says nothing extra about a section that is declared", async () => {
    serve(
      {
        "/domains/eng/manifest": () => ({
          domain: "eng",
          markdown: MANIFEST,
          sections: sectionsResponse({
            tag_aliases: {
              decls: [{ alias: "k8s", canonical: "kubernetes" }],
              problems: [],
            },
          }),
        }),
      },
      "admin",
    );

    renderApp("/d/eng");

    const aliases = await screen.findByRole("region", { name: "Tag aliases" });
    expect(within(aliases).getByText("k8s -> kubernetes")).toBeVisible();
    // Somebody who has already configured a thing does not need to be told
    // what it is, and has nothing to start.
    expect(
      within(aliases).queryByText(/Spellings that fold into one canonical tag/),
    ).toBeNull();
    expect(
      within(aliases).queryByRole("button", { name: "Add Tag Aliases" }),
    ).toBeNull();
  });

  it("shows the explanation but no action to a reader who cannot edit", async () => {
    serve();

    renderApp("/d/eng");

    // The honest answer for somebody without the right: they can still learn
    // what the section does, they just cannot start it from here.
    const aliases = await screen.findByRole("region", { name: "Tag aliases" });
    expect(
      within(aliases).getByText(/Spellings that fold into one canonical tag/),
    ).toBeVisible();
    expect(
      within(aliases).queryByRole("button", { name: "Add Tag Aliases" }),
    ).toBeNull();
  });

  it("draws the old bare line for a daemon that sends no startable sections", async () => {
    serve(
      {
        "/domains/eng/manifest": () => ({
          domain: "eng",
          markdown: MANIFEST,
          sections: sectionsResponse({ starters: [] }),
        }),
      },
      "admin",
    );

    renderApp("/d/eng");

    // Nothing to explain a section with, so nothing is invented on this side.
    const aliases = await screen.findByRole("region", { name: "Tag aliases" });
    expect(within(aliases).getByText("No aliases")).toBeVisible();
    expect(
      within(aliases).queryByRole("button", { name: "Add Tag Aliases" }),
    ).toBeNull();
  });

  it("shows the whole configurable surface for a domain whose MANIFEST is blank", async () => {
    serve(
      {
        "/domains/eng/manifest": () => ({
          domain: "eng",
          markdown: "",
          sections: sectionsResponse({
            scope: [],
            when_to_use: [],
            routing: "none",
            missing: ["Scope", "When to Use"],
            policies: [],
          }),
        }),
      },
      "admin",
    );

    renderApp("/d/eng");

    // The gap is still named, and every box below it is now something the
    // reader can see, understand and start.
    expect(await screen.findByText(/no MANIFEST yet/)).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Add When to Use" }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "Add Scope" })).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Add Provisioning" }),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Create a MANIFEST" }),
    ).toBeVisible();
  });

  it("offers no seeding action over a MANIFEST nobody could read", async () => {
    serve(
      {
        "/domains/eng/manifest": () => ({
          domain: "eng",
          markdown: MANIFEST,
          sections: sectionsResponse({ policies: [] }),
        }),
      },
      "admin",
    );

    renderApp("/d/eng");

    // The sections are still explained - the registry does not depend on this
    // document - and the document itself is still drawn, because repairing it
    // is what somebody does next.
    const aliases = await screen.findByRole("region", { name: "Tag aliases" });
    expect(
      within(aliases).getByText(/Spellings that fold into one canonical tag/),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Create a MANIFEST" }),
    ).toBeNull();
  });

  it("carries the section to the editor when the action is pressed", async () => {
    serve({ "/validate": () => ({ findings: [], errors: 0 }) }, "admin");

    renderApp("/d/eng");

    await userEvent.click(
      await screen.findByRole("button", { name: "Add Tag Aliases" }),
    );

    // The whole hop, end to end: the button names a section, the navigation
    // state carries the name, and the editor puts the server's own example
    // for it into the buffer. The two ends are only ever tested together.
    expect(
      await screen.findByRole("heading", { name: "Editing eng MANIFEST" }),
    ).toBeVisible();
    const editor = await screen.findByLabelText("MANIFEST source");
    await waitFor(() => {
      expect(editor.textContent).toContain("## Tag Aliases");
    });
    expect(editor.textContent).toContain("k8s -> kubernetes");
  });

  it("wears a private badge beside its name when the domain is private, and none when it is shared", async () => {
    serve({
      "/domains": () => ({
        behavior: [],
        domains: [
          {
            name: "eng",
            kind: "file",
            engrams: 4,
            private: true,
            when_to_use: ["Route here for eng questions."],
          },
        ],
      }),
    });

    renderApp("/d/eng");

    // A sibling of the heading rather than inside it, off the listing every
    // other chip on this header draws from, so the heading's own accessible
    // name stays exactly "eng".
    const heading = await screen.findByRole("heading", { name: "eng" });
    expect(heading).toBeVisible();
    await waitFor(() => {
      expect(heading.parentElement).toHaveTextContent("private");
    });
  });

  it("wears the badge even when the membership read never lands", async () => {
    // The badge is the listing's answer, not the membership card's: a
    // members read that fails takes the card down with it and leaves the
    // header saying exactly what it said before.
    serve({
      "/domains": () => ({
        behavior: [],
        domains: [
          {
            name: "eng",
            kind: "file",
            engrams: 4,
            private: true,
            when_to_use: [],
          },
        ],
      }),
      "/domains/eng/members": () => {
        throw new ApiProblem(500, "internal", "the accounts store is down");
      },
    });

    renderApp("/d/eng");

    const heading = await screen.findByRole("heading", { name: "eng" });
    await waitFor(() => {
      expect(heading.parentElement).toHaveTextContent("private");
    });
  });

  it("tells a member how much of their own work is waiting in a reviewing domain", async () => {
    // An editor, so `can_share` is false and neither share card is drawn: this
    // is the person whose count used to be invisible in the browser, because
    // the only place that carried it was the sync route, which is gated with
    // the share verbs. The listing is the read every member already makes.
    serve({
      "/domains": () => ({
        behavior: [],
        domains: [
          {
            name: "eng",
            kind: "file",
            engrams: 4,
            review: "overlay",
            my_drafts: 2,
            when_to_use: ["Route here for eng questions."],
          },
        ],
      }),
    });

    renderApp("/d/eng");

    expect(
      await screen.findByText(/You have 2 draft changes here/),
    ).toBeVisible();
    // No share card on this session, so the header is the whole of what says
    // it.
    expect(screen.queryByRole("region", { name: "Team sync" })).toBeNull();
  });

  it("says nothing about drafts on a domain that takes changes directly", async () => {
    serve();

    renderApp("/d/eng");

    expect(await screen.findByRole("heading", { name: "eng" })).toBeVisible();
    expect(screen.queryByText(/draft changes here/)).toBeNull();
  });

  it("wears no private badge when the domain is shared", async () => {
    serve();

    renderApp("/d/eng");

    expect(await screen.findByRole("heading", { name: "eng" })).toBeVisible();
    expect(screen.queryByText("private")).toBeNull();
  });

  it("renders a MANIFEST that is only a title without calling it missing", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: "---\ntitle: eng\n---\n\n# eng\n",
      }),
    });

    renderApp("/d/eng");

    // A document with nothing but its title is still a document: it is not
    // the same fact as a missing MANIFEST.
    await screen.findByRole("region", { name: "Manifest" });
    await within(await screenBody()).findByRole("link", { name: /Alpha/ });
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

  it("offers an admin the editor over a manifest that is not there yet", async () => {
    serve(
      {
        "/domains/eng/manifest": () => {
          throw new ApiProblem(404, "not found", "no MANIFEST in domain 'eng'");
        },
      },
      "admin",
    );

    renderApp("/d/eng");

    // A domain with no MANIFEST is exactly the domain an admin opens the
    // editor to fix, so the gap sentence comes with the way to close it.
    const section = await screen.findByRole("region", { name: "Manifest" });
    await within(section).findByText(/no MANIFEST yet/);
    expect(
      within(section).getByRole("link", { name: "Edit MANIFEST" }),
    ).toHaveAttribute("href", "/d/eng/manifest/edit");
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

  it("shows a folder as its own page: the trail, the count, and none of the domain's furniture", async () => {
    serve(
      {
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
        "/domains/eng/sync": () => syncResponse(),
      },
      "admin",
    );

    // Straight to the folder, because the whole of this screen's state is its
    // URL: the same link somebody sends, and the same address the back button
    // returns to.
    renderApp("/d/eng?path=notes");
    const body = await screenBody();

    // The heading is the folder's path: the domain links to its page, the
    // folder itself is plain text.
    const heading = await within(body).findByRole("heading", {
      level: 1,
      name: "eng / notes",
    });
    expect(within(heading).getByRole("link", { name: "eng" })).toHaveAttribute(
      "href",
      "/d/eng",
    );
    expect(within(heading).queryByRole("link", { name: "notes" })).toBeNull();
    // All of them: the list pages towards the envelope's six hundred and the
    // stub answers every page with the same row, so what is pinned here is
    // that the folder's engrams are listed, not how many pages landed first.
    const rows = await within(body).findAllByRole("link", { name: /Beta/ });
    expect(rows[0]).toBeVisible();
    // The listing endpoint, scoped and paged, rather than the tree's own
    // rows: a folder of six hundred engrams costs one page here.
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
    // The count is the envelope's, which counts the subtree, and it stands
    // under the heading once.
    expect(
      await within(body).findByText("620 engrams in this folder"),
    ).toBeVisible();
    expect(
      within(body).getByText(/Browsing notes, subfolders included/),
    ).toBeVisible();
    // What a folder page carries: New engram and the order menu.
    expect(
      within(body).getByRole("button", { name: "New engram" }),
    ).toBeVisible();
    expect(
      within(body).getByRole("button", { name: "Order: Newest first" }),
    ).toBeVisible();
    // And what it does not, as an admin on a team domain, where the domain
    // page would draw every one of these.
    expect(
      within(body).queryByRole("region", { name: "Team sync" }),
    ).toBeNull();
    expect(
      within(body).queryByRole("region", { name: "Proposals" }),
    ).toBeNull();
    expect(within(body).queryByRole("region", { name: "Members" })).toBeNull();
    expect(
      within(body).queryByRole("region", { name: "Review mode" }),
    ).toBeNull();
    expect(within(body).queryByRole("region", { name: "Manifest" })).toBeNull();
    expect(within(body).queryByRole("region", { name: "Backup" })).toBeNull();
    expect(
      within(body).queryByRole("region", { name: "Danger zone" }),
    ).toBeNull();
    expect(
      within(body).queryByRole("link", { name: "Download archive" }),
    ).toBeNull();
    expect(
      within(body).queryByRole("button", { name: "Import archive" }),
    ).toBeNull();
    expect(
      within(body).queryByRole("button", { name: "Unregister domain" }),
    ).toBeNull();
    expect(within(body).queryByText(/engrams$/)).toBeNull();
  });

  it("walks a nested folder's trail back out, one link per parent", async () => {
    serve({
      "/domains/eng/tree": () => ({
        domain: "eng",
        path: "notes/deep",
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

    renderApp("/d/eng?path=notes%2Fdeep");
    const body = await screenBody();

    const heading = await within(body).findByRole("heading", {
      level: 1,
      name: "eng / notes / deep",
    });
    expect(
      within(heading).getByRole("link", { name: "notes" }),
    ).toHaveAttribute("href", "/d/eng?path=notes");
    expect(within(heading).queryByRole("link", { name: "deep" })).toBeNull();
    expect(
      await within(body).findByText("0 engrams in this folder"),
    ).toBeVisible();
    expect(within(body).getByText("This folder has no engrams.")).toBeVisible();
  });

  it("orders the listing the way the reader chose, and remembers it", async () => {
    serve();

    renderApp("/d/eng");
    const body = await screenBody();
    await within(body).findByRole("link", { name: /Alpha/ });

    await userEvent.click(
      within(body).getByRole("button", { name: "Order: Newest first" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitemradio", { name: "Name A to Z" }),
    );

    // The trigger wears the choice, the request carries it, and the browser
    // keeps it.
    expect(
      await within(body).findByRole("button", { name: "Order: Name A to Z" }),
    ).toBeVisible();
    await waitFor(() => {
      expect(
        requested().some(
          (path) =>
            path.startsWith("/domains/eng/engrams?") &&
            path.includes("sort=path") &&
            path.includes("dir=asc"),
        ),
      ).toBe(true);
    });
    expect(localStorage.getItem("fluid.engrams.order")).toBe("name-asc");
  });

  it("reads a remembered order at mount", async () => {
    localStorage.setItem("fluid.engrams.order", "oldest");
    serve();

    renderApp("/d/eng");
    const body = await screenBody();

    expect(
      await within(body).findByRole("button", { name: "Order: Oldest first" }),
    ).toBeVisible();
    await waitFor(() => {
      expect(
        requested().some(
          (path) =>
            path.startsWith("/domains/eng/engrams?") &&
            path.includes("sort=recorded") &&
            path.includes("dir=asc"),
        ),
      ).toBe(true);
    });
  });

  it("applies the order to a filtered listing too, and never to a search", async () => {
    localStorage.setItem("fluid.engrams.order", "name-desc");
    serve();

    renderApp("/d/eng?tags=eng");
    const body = await screenBody();
    await screen.findByRole("link", { name: /Gamma/ });

    expect(
      within(body).getByRole("button", { name: "Order: Name Z to A" }),
    ).toBeVisible();
    const filtered = requested().filter(
      (path) =>
        path.startsWith("/domains/eng/engrams?") && path.includes("tags=eng"),
    );
    expect(filtered.length).toBeGreaterThan(0);
    expect(
      filtered.every(
        (path) => path.includes("sort=path") && path.includes("dir=desc"),
      ),
    ).toBe(true);
    // Every listing request carries the order, not only the ones the filter
    // is visible on: a request that went out before the filter resolved
    // would otherwise slip through unordered.
    const listings = requested().filter((path) =>
      path.startsWith("/domains/eng/engrams?"),
    );
    expect(listings.length).toBeGreaterThan(0);
    expect(
      listings.every(
        (path) => path.includes("sort=path") && path.includes("dir=desc"),
      ),
    ).toBe(true);
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
    // Still the folder page: a filter keeps the page it is on.
    expect(
      screen.getByRole("heading", { level: 1, name: "eng / notes" }),
    ).toBeVisible();
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

  it("draws the status line a discard arrives with", async () => {
    serve();

    // Where a discarded addition lands: the engram page it was standing on
    // is gone, so the page it came back to is the one that says what
    // happened to it.
    render(
      <MemoryRouter
        initialEntries={[
          { pathname: "/d/eng", state: { discarded: "alpha.md" } },
        ]}
      >
        <App />
      </MemoryRouter>,
    );

    await screen.findByRole("heading", { level: 1, name: "eng" });
    // Named by its text rather than by its role alone: a screen that is still
    // loading a panel carries a busy region of the same role.
    expect(await screen.findByText("Discarded alpha.md.")).toHaveAttribute(
      "role",
      "status",
    );
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
    // The instant, parsed into this machine's own local date and time (built
    // off the same Date the component parses, since the suite may run in any
    // zone), with how long ago it was after it - the relative phrase is left
    // to a wildcard since it moves with the wall clock the test runs against.
    const parsed = new Date("2026-08-10T08:00:00Z");
    const pad = (n: number) => String(n).padStart(2, "0");
    const instant = `${String(parsed.getFullYear())}-${pad(parsed.getMonth() + 1)}-${pad(parsed.getDate())} ${pad(parsed.getHours())}:${pad(parsed.getMinutes())}`;
    expect(
      within(card).getByText(new RegExp(`^${instant}, .+ ago$`)),
    ).toBeVisible();
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
    // Built off the same Date the component parses, so the local date and
    // time read the same here as they do on whatever machine runs the suite;
    // the relative phrase is left to a wildcard since it moves with the wall
    // clock the test runs against.
    const parsed = new Date("2026-08-09T08:00:00Z");
    const pad = (n: number) => String(n).padStart(2, "0");
    const instant = `${String(parsed.getFullYear())}-${pad(parsed.getMonth() + 1)}-${pad(parsed.getDate())} ${pad(parsed.getHours())}:${pad(parsed.getMinutes())}`;
    expect(
      within(card).getByText(new RegExp(`^${instant}, .+ ago \\(stale\\)$`)),
    ).toBeVisible();
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

    // On an instance that shares as itself the endpoints are admin-only, so an
    // editor's screen must not knock on them at all: a 403 in the console is
    // noise nobody can act on. The probe is what says so - the screen never
    // asks a domain's origin whether it is allowed to ask.
    expect(
      requested().some((path) => path.startsWith("/domains/eng/sync")),
    ).toBe(false);
    expect(
      within(body).queryByRole("region", { name: "Team sync" }),
    ).toBeNull();
    expect(
      within(body).queryByRole("region", { name: "Proposals" }),
    ).toBeNull();
  });

  it("gives an editor both cards where the instance shares as they do", async () => {
    serve(
      {
        "/domains/eng/sync": () =>
          syncResponse({
            open_proposals: [],
            declined_proposals: [],
            conflicts: [],
            connection: {
              connected: true,
              user: "octo",
              token_store: "keychain",
              share_identity: "personal",
            },
          }),
        "/domains/eng/sync/changes": () => ({
          action: "create",
          effective_title: "Share 1 new engram from eng",
          changes: [{ path: "notes/a.md", kind: "added" }],
        }),
        // This session has connected nothing yet, which is what the dialog
        // has something to say about.
        "/me/github-identity": () => ({
          account: "ada",
          connected: false,
          login: null,
          connected_at: null,
          token_store: null,
          pending: null,
          error: null,
        }),
      },
      "editor",
      () =>
        meResponse({ user: userFixture({ role: "editor" }), can_share: true }),
    );

    renderApp("/d/eng");
    const body = await screenBody();

    // Both cards, because both routes behind them serve this account now.
    const proposals = await within(body).findByRole("region", {
      name: "Proposals",
    });
    expect(
      await within(body).findByRole("region", { name: "Team sync" }),
    ).toBeVisible();

    // And the identity affordance the dialogs carry: an editor sharing on
    // their own name is exactly the session that may not have connected one.
    await userEvent.click(
      within(proposals).getByRole("button", { name: "Share changes" }),
    );
    const dialog = await screen.findByRole("dialog", { name: /share/i });
    expect(
      await within(dialog).findByRole("link", {
        name: "Connect GitHub to share",
      }),
    ).toHaveAttribute("href", "/profile");
  });

  it("keeps an admin's cards whichever identity the instance shares as", async () => {
    serve(
      {
        "/domains/eng/sync": () =>
          syncResponse({
            connection: {
              connected: true,
              user: "octo",
              token_store: "keychain",
              share_identity: "personal",
            },
          }),
        "/me/github-identity": () => ({
          account: "root",
          connected: true,
          login: "octo",
          connected_at: "2026-08-29T09:12:44Z",
          token_store: "keyring",
          pending: null,
          error: null,
        }),
      },
      "admin",
    );

    renderApp("/d/eng");
    const body = await screenBody();

    // Personal mode widens who may share; it never narrows it.
    expect(
      await within(body).findByRole("region", { name: "Team sync" }),
    ).toBeVisible();
    expect(
      await within(body).findByRole("region", { name: "Proposals" }),
    ).toBeVisible();
  });

  it("keeps an admin's cards when the probe says nothing about sharing", async () => {
    serve(
      { "/domains/eng/sync": () => syncResponse() },
      "admin",
      olderProbe("admin"),
    );

    renderApp("/d/eng");

    // A server that predates the field draws the screens it always drew.
    expect(
      await within(await screenBody()).findByRole("region", {
        name: "Team sync",
      }),
    ).toBeVisible();
  });

  it("gives an editor nothing when the probe says nothing about sharing", async () => {
    serve(
      { "/domains/eng/sync": () => syncResponse() },
      "editor",
      olderProbe("editor"),
    );

    renderApp("/d/eng");
    const body = await screenBody();
    await within(body).findByRole("link", { name: /Alpha/ });

    // The other half of the same fallback: an absent answer is read as the
    // rule that held before there was one, so nothing is widened by silence.
    expect(
      within(body).queryByRole("region", { name: "Team sync" }),
    ).toBeNull();
    expect(
      requested().some((path) => path.startsWith("/domains/eng/sync")),
    ).toBe(false);
  });
});

/**
 * One domain listing row, in the engine's own wire shape, with the stable
 * name fields Task 21 added: `canonical_name`, `aliases`, `name_origin`,
 * `shadowed` and `renaming`. A function rather than a constant so a test
 * maps over a fresh copy, the way the policy rows above do.
 */
function domainRow(overrides: Record<string, unknown> = {}) {
  return {
    name: "eng",
    kind: "file",
    engrams: 4,
    when_to_use: ["Route here for eng questions."],
    canonical_name: "eng",
    aliases: [],
    name_origin: "derived",
    shadowed: false,
    renaming: false,
    ...overrides,
  };
}

function listingOf(...rows: Record<string, unknown>[]) {
  return {
    behavior: ["Search before answering from memory."],
    domains: rows,
  };
}

describe("the domain's declared name", () => {
  it("says nothing extra over an ordinary domain", async () => {
    serve();

    renderApp("/d/eng");

    await screen.findByRole("heading", { level: 1, name: "eng" });
    expect(screen.queryByText(/^Known everywhere as/)).toBeNull();
    expect(screen.queryByText(/^Former names:/)).toBeNull();
    expect(screen.queryByText(/^This domain calls itself/)).toBeNull();
  });

  it("names the canonical name and every former name in the header", async () => {
    serve({
      "/domains": () =>
        listingOf(
          domainRow({
            canonical_name: "engineering",
            aliases: ["old-eng", "eng-legacy"],
          }),
        ),
    });

    renderApp("/d/eng");

    expect(
      await screen.findByText("Known everywhere as engineering"),
    ).toBeVisible();
    expect(screen.getByText("Former names: old-eng, eng-legacy")).toBeVisible();
  });

  it("shows the shadowed banner and its Rename action to an admin", async () => {
    serve(
      {
        "/domains": () =>
          listingOf(domainRow({ canonical_name: "knowledge", shadowed: true })),
      },
      "admin",
    );

    renderApp("/d/eng");

    const sentence = await screen.findByText(
      "This domain calls itself 'knowledge', but 'knowledge' is another domain here, so links that name 'knowledge' reach that one. Rename one of them to line them up.",
    );
    const banner = sentence.closest('[role="status"]');
    expect(banner).not.toBeNull();
    expect(
      within(banner as HTMLElement).getByRole("button", { name: "Rename" }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "Rename domain" })).toBeVisible();
  });

  it("keeps Rename domain inert on a read-only instance, and says why", async () => {
    serve({}, "admin", () =>
      meResponse({
        user: userFixture({ role: "admin" }),
        read_only: true,
      }),
    );

    renderApp("/d/eng");

    const trigger = await screen.findByRole("button", {
      name: "Rename domain",
    });
    expect(trigger).not.toBeDisabled();
    expect(trigger).toHaveAttribute("aria-disabled", "true");
    expect(trigger).toHaveAccessibleDescription(
      "This instance is read only, so nothing here can be changed.",
    );

    await userEvent.click(trigger);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(
      requested().some((path) => path.startsWith("/domains/eng/rename")),
    ).toBe(false);
  });

  it("hides both rename controls from a plain member of a private domain", async () => {
    serve(
      {
        "/domains": () =>
          listingOf(
            domainRow({
              canonical_name: "knowledge",
              shadowed: true,
              private: true,
            }),
          ),
        "/domains/eng/members": () => ({
          owner: "grace",
          visibility: "private",
          members: [],
        }),
      },
      "editor",
    );

    renderApp("/d/eng");

    await screen.findByText(/^This domain calls itself/);
    expect(screen.queryByRole("button", { name: "Rename domain" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Rename" })).toBeNull();
  });

  it("shows Renaming... and disables every write control, each with the same reason", async () => {
    serve(
      {
        "/domains": () => listingOf(domainRow({ renaming: true })),
      },
      "admin",
    );

    renderApp("/d/eng");

    expect(await screen.findByText("Renaming...")).toHaveAttribute(
      "role",
      "status",
    );

    const REASON = "A rename of this domain is already running.";

    // New engram: `aria-disabled`, not the native attribute, the same trade
    // every other control on this app makes - a control taken out of the tab
    // order could never have its reason heard.
    const newEngram = screen.getByRole("button", { name: "New engram" });
    expect(newEngram).not.toBeDisabled();
    expect(newEngram).toHaveAttribute("aria-disabled", "true");
    expect(newEngram).toHaveAccessibleDescription(REASON);

    // The launcher itself: the same reason as every other control here,
    // not the read-only one - a rename in progress is what is disabling it.
    const renameLauncher = screen.getByRole("button", {
      name: "Rename domain",
    });
    expect(renameLauncher).not.toBeDisabled();
    expect(renameLauncher).toHaveAttribute("aria-disabled", "true");
    expect(renameLauncher).toHaveAccessibleDescription(REASON);

    const editManifest = await screen.findByRole("link", {
      name: "Edit MANIFEST",
    });
    expect(editManifest).toHaveAttribute("aria-disabled", "true");
    expect(editManifest).toHaveAccessibleDescription(REASON);

    const importArchive = await screen.findByRole("button", {
      name: "Import archive",
    });
    expect(importArchive).toHaveAttribute("aria-disabled", "true");
    expect(importArchive).toHaveAccessibleDescription(REASON);

    const unregister = await screen.findByRole("button", {
      name: "Unregister domain",
    });
    expect(unregister).toHaveAttribute("aria-disabled", "true");
    expect(unregister).toHaveAccessibleDescription(REASON);

    // Gated on the members read landing, the way the card itself gates it.
    const share = await screen.findByRole("button", { name: "Make private" });
    expect(share).toHaveAttribute("aria-disabled", "true");
    expect(share).toHaveAccessibleDescription(REASON);

    // A click on any of them does nothing: guarded at the press, not merely
    // painted as inert. New engram, Import archive and the launcher each
    // open a dialog when they act, and Edit MANIFEST navigates to the
    // editor; none of the four does either.
    await userEvent.click(newEngram);
    await userEvent.click(renameLauncher);
    await userEvent.click(editManifest);
    await userEvent.click(importArchive);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByRole("heading", { name: /^Editing/ })).toBeNull();
  });

  it("polls only while THIS domain is renaming, and stops once it clears", async () => {
    let renaming = true;
    serve({
      "/domains": () => listingOf(domainRow({ renaming })),
    });

    renderApp("/d/eng");
    await screen.findByText("Renaming...");

    const before = requested().filter((path) => path === "/domains").length;
    renaming = false;
    await waitFor(() => {
      expect(
        requested().filter((path) => path === "/domains").length,
      ).toBeGreaterThan(before);
    });
    await waitFor(() => {
      expect(screen.queryByText("Renaming...")).toBeNull();
    });

    // Once it has cleared, waiting a poll interval brings no further read:
    // the interval turned itself off rather than continuing to ask.
    const settled = requested().filter((path) => path === "/domains").length;
    await new Promise((resolve) => setTimeout(resolve, 1100));
    expect(requested().filter((path) => path === "/domains").length).toBe(
      settled,
    );
  });

  it("resumes the poll on its own once a later fetch of the listing succeeds", async () => {
    // Answered a refusal a real one would not retry on its own (`isDecided`
    // in `query/client.ts`), so the query settles into an error state after
    // exactly one failed attempt rather than this app's own retry policy
    // spending several before it gives up.
    let mode: "ok" | "fail" = "ok";
    serve({
      "/domains": () => {
        if (mode === "fail") {
          throw new ApiProblem(422, "invalid", "temporary failure");
        }
        return listingOf(domainRow({ renaming: true }));
      },
    });

    renderApp("/d/eng");
    await screen.findByText("Renaming...");

    const domainsReads = () =>
      requested().filter((path) => path === "/domains").length;
    const before = domainsReads();
    mode = "fail";
    // A window-focus refetch, standing in for whatever asks the listing
    // again first: the query's own `refetchInterval` re-evaluates on every
    // settle regardless of what triggered it, and a failed one turns
    // polling off (`query.state.status === "error"`).
    window.dispatchEvent(new Event("visibilitychange"));
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("focus"));
    await waitFor(() => {
      expect(domainsReads()).toBeGreaterThan(before);
    });
    const afterFailure = domainsReads();
    await new Promise((resolve) => setTimeout(resolve, 1100));
    expect(domainsReads()).toBe(afterFailure);

    // A later fetch that succeeds - again standing in for anything that
    // would ask, a focus refetch here - resumes the once-a-second poll on
    // its own: nothing needed to be told to restart it, since
    // `refetchInterval` reads `renaming: true` off this very read and
    // schedules the next one exactly as it would have all along. Waited
    // for two reads beyond the failure, not one: the first is the recovery
    // fetch itself succeeding, and only the second is the interval having
    // actually resumed and fired again on its own.
    mode = "ok";
    window.dispatchEvent(new Event("visibilitychange"));
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("focus"));
    await waitFor(
      () => {
        expect(domainsReads()).toBeGreaterThan(afterFailure + 1);
      },
      { timeout: 3000 },
    );
  });

  it("offers the rename controls to a non-admin owner of a private domain", async () => {
    serve(
      {
        "/domains": () =>
          listingOf(domainRow({ canonical_name: "eng", private: true })),
        "/domains/eng/members": () => ({
          owner: "ada",
          visibility: "private",
          members: [],
        }),
      },
      "editor",
    );

    renderApp("/d/eng");

    expect(
      await screen.findByRole("button", { name: "Rename domain" }),
    ).toBeVisible();
  });

  it("opens the rename dialog from the shadowed banner's own action", async () => {
    serve(
      {
        "/domains": () =>
          listingOf(domainRow({ canonical_name: "knowledge", shadowed: true })),
      },
      "admin",
    );

    renderApp("/d/eng");

    const sentence = await screen.findByText(/^This domain calls itself/);
    const banner = sentence.closest('[role="status"]') as HTMLElement;
    await userEvent.click(
      within(banner).getByRole("button", { name: "Rename" }),
    );

    expect(
      await screen.findByRole("dialog", { name: "Rename domain" }),
    ).toBeVisible();
  });
});

describe("a domain's old address", () => {
  /** The one graph node the neighborhood section needs to draw something. */
  function soloGraph(domain: string, permalink: string, title: string) {
    return {
      nodes: [
        { id: 1, domain, permalink, title, status: "stable", type: "engram" },
      ],
      edges: [],
      truncated: false,
    };
  }

  function runbookDetail() {
    return {
      domain: "eng-knowledge",
      permalink: "runbook",
      title: "Runbook",
      type: "engram",
      status: "stable",
      path: "runbook.md",
      url: "crystalline://eng-knowledge/runbook",
      content: [
        "---",
        "title: Runbook",
        "---",
        "",
        "# Runbook",
        "",
        "How to run it.",
        "",
      ].join("\n"),
      checksum: "abc123",
      frontmatter: {
        engram_type: "engram",
        title: "Runbook",
        permalink: "runbook",
        status: "stable",
        tags: [],
        extra: {},
        valid_from: null,
        valid_to: null,
        stale_after: null,
        verified: [],
        last_verified: null,
        review_after: null,
        recorded_at: null,
      },
      observations: [],
      relations: [],
      links: [],
      inbound: { count: 0, refs: [] },
    };
  }

  it("sends an engram address under an old local name to the domain's local name now", async () => {
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture({ role: "editor" }) }),
        "/domains": () =>
          listingOf(
            domainRow({
              name: "eng-knowledge",
              canonical_name: "eng-knowledge",
              aliases: ["old-eng"],
              name_origin: "explicit",
            }),
          ),
        // The sidebar's own tree read fires for whatever domain the address
        // names before the redirect lands, so both are stubbed.
        "/domains/old-eng/tree": () => ({
          domain: "eng-knowledge",
          path: "/",
          folders: [],
          engrams: [],
        }),
        "/domains/eng-knowledge/tree": () => ({
          domain: "eng-knowledge",
          path: "/",
          folders: [],
          engrams: [
            {
              permalink: "runbook",
              title: "Runbook",
              type: "engram",
              path: "runbook.md",
            },
          ],
        }),
        "/domains/eng-knowledge/engrams/runbook": runbookDetail,
        "/graph": () => soloGraph("eng-knowledge", "runbook", "Runbook"),
      }),
    );

    renderApp("/d/old-eng/e/runbook");

    expect(
      await screen.findByRole("heading", { name: "Runbook" }),
    ).toBeVisible();
  });

  it("shows today's not-found state for a segment nobody answers to", async () => {
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture({ role: "editor" }) }),
        "/domains": () =>
          listingOf(
            domainRow({
              name: "eng-knowledge",
              canonical_name: "eng-knowledge",
              aliases: ["old-eng"],
              name_origin: "explicit",
            }),
          ),
      }),
    );

    renderApp("/d/nope");

    expect(
      await screen.findByRole("heading", { name: "Domain not found" }),
    ).toBeVisible();
  });

  it("keeps the query string and the hash across the redirect", async () => {
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture({ role: "editor" }) }),
        "/domains": () =>
          listingOf(
            domainRow({
              name: "eng-knowledge",
              canonical_name: "eng-knowledge",
              aliases: ["old-eng"],
              name_origin: "explicit",
            }),
          ),
        "/domains/eng-knowledge/manifest": () => ({
          domain: "eng-knowledge",
          markdown: "# eng-knowledge",
        }),
        "/domains/eng-knowledge/tree": () => ({
          domain: "eng-knowledge",
          path: "/",
          folders: [],
          engrams: [],
        }),
        "/domains/eng-knowledge/engrams": () => ({
          mode: "text",
          total: 0,
          page: 1,
          limit: 50,
          count: 0,
          hits: [],
        }),
        "/vocabulary": () => ({
          domain: "eng-knowledge",
          tags: [],
          categories: [],
          relation_types: [],
        }),
        "/domains/eng-knowledge/members": () => ({
          owner: null,
          visibility: "shared",
          members: [],
        }),
      }),
    );

    /** A sibling of `App`, under the same in-memory history, reading it. */
    function LocationProbe() {
      const location = useLocation();
      return (
        <div data-testid="probe">
          {`${location.pathname}${location.search}${location.hash}`}
        </div>
      );
    }

    render(
      <MemoryRouter initialEntries={["/d/old-eng?tab=activity#section-2"]}>
        <App />
        <LocationProbe />
      </MemoryRouter>,
    );

    await screen.findByRole("heading", { level: 1, name: "eng-knowledge" });
    await waitFor(() => {
      expect(screen.getByTestId("probe")).toHaveTextContent(
        "/d/eng-knowledge?tab=activity#section-2",
      );
    });
  });

  it("tries again after a failed confirming fetch, once the listing is asked again", async () => {
    let call = 0;
    const listing = () =>
      listingOf(
        domainRow({
          name: "eng-knowledge",
          canonical_name: "eng-knowledge",
          aliases: ["old-eng"],
          name_origin: "explicit",
        }),
      );

    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture({ role: "editor" }) }),
        "/domains": () => {
          call += 1;
          // The second call is the redirect's own confirming `fetchQuery`,
          // off the cache-level match the first (ordinary) read already
          // supplied. Answered with a refusal a real one would not retry
          // on its own (`isDecided` in `query/client.ts`), so this is one
          // clean failed attempt rather than this app's own retry policy
          // quietly turning it into several.
          if (call === 2) {
            throw new ApiProblem(422, "invalid", "temporary failure");
          }
          return listing();
        },
        "/domains/eng-knowledge/manifest": () => ({
          domain: "eng-knowledge",
          markdown: "# eng-knowledge",
        }),
        "/domains/eng-knowledge/tree": () => ({
          domain: "eng-knowledge",
          path: "/",
          folders: [],
          engrams: [],
        }),
        "/domains/eng-knowledge/engrams": () => ({
          mode: "text",
          total: 0,
          page: 1,
          limit: 50,
          count: 0,
          hits: [],
        }),
        "/vocabulary": () => ({
          domain: "eng-knowledge",
          tags: [],
          categories: [],
          relation_types: [],
        }),
        "/domains/eng-knowledge/members": () => ({
          owner: null,
          visibility: "shared",
          members: [],
        }),
      }),
    );

    renderApp("/d/old-eng");

    // The confirming fetch (the second read) has failed, and nothing
    // redirects off a failure: still the not-found state for the address
    // as typed, since "old-eng" answers to nothing under its own name.
    await waitFor(() => {
      expect(call).toBeGreaterThanOrEqual(2);
    });
    expect(
      screen.getByRole("heading", { name: "Domain not found" }),
    ).toBeVisible();

    // A later read of the listing - a window-focus refetch here, standing
    // in for any of the several ordinary things that would ask again - is
    // what the failure cleared the guard for: this segment gets a fresh
    // confirming fetch of its own rather than staying disabled for the
    // rest of the session.
    window.dispatchEvent(new Event("visibilitychange"));
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("focus"));

    expect(
      await screen.findByRole("heading", { level: 1, name: "eng-knowledge" }),
    ).toBeVisible();
  });
});
