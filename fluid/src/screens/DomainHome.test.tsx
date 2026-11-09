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
import type { Answer } from "../test/harness";
import {
  answersFor,
  defaultPolicyRows,
  defaultStarterRows,
  domainsResponse,
  manifestSectionsResponse,
  meResponse,
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

/**
 * Open the front page's More actions menu and hand back its rows.
 */
async function openActions(): Promise<HTMLElement> {
  const body = await screenBody();
  await userEvent.click(
    await within(body).findByRole("button", { name: "More actions" }),
  );
  return screen.findByRole("menu");
}

beforeEach(() => {
  apiMock.mockReset();
  // The listing's order is remembered across sessions, so each test starts
  // from a browser that has never been told anything.
  localStorage.clear();
});

describe("the domain screen", () => {
  it("a locked domain's home wears the lock and offers no New engram", async () => {
    serve(
      {
        "/domains": () => ({
          ...domainsResponse(),
          domains: [{ ...domainsResponse().domains[0], read_only: true }],
        }),
        "/domains/eng/manifest": () => ({
          domain: "eng",
          markdown: "# eng",
          sections: manifestSectionsResponse({
            policies: defaultPolicyRows().map((row) =>
              row.key === "read_only"
                ? { ...row, declared: "on", effective: "on" }
                : row,
            ),
          }),
        }),
      },
      "admin",
    );
    renderApp("/d/eng");
    const body = await screenBody();
    expect(await within(body).findByText("read-only")).toBeInTheDocument();
    expect(
      within(body).queryByRole("button", { name: "New engram" }),
    ).not.toBeInTheDocument();
  });

  it("a domain off this caller's write list offers no New engram and wears no lock", async () => {
    serve(
      {
        "/domains": () => ({
          ...domainsResponse(),
          domains: [{ ...domainsResponse().domains[0], read_only: true }],
        }),
      },
      "admin",
    );
    renderApp("/d/eng");
    const body = await screenBody();
    await within(body).findByRole("heading", { level: 1 });
    await waitFor(() => {
      expect(requested()).toContain("/domains");
    });
    await waitFor(() => {
      expect(
        within(body).queryByRole("button", { name: "New engram" }),
      ).not.toBeInTheDocument();
    });
    expect(within(body).queryByText("read-only")).not.toBeInTheDocument();
  });

  /** The MANIFEST of `eng` with its `read_only` row on. */
  function lockedManifest(): unknown {
    return {
      domain: "eng",
      markdown: "# eng",
      sections: manifestSectionsResponse({
        policies: defaultPolicyRows().map((row) =>
          row.key === "read_only"
            ? { ...row, declared: "on", effective: "on" }
            : row,
        ),
      }),
    };
  }

  /**
   * Whether the MANIFEST editor, the seed buttons and Import archive are
   * offered, on the page, in its More actions menu and in the palette.
   * Download archive is a read and is always offered to an admin.
   */
  async function expectManifestWrites(offered: boolean): Promise<void> {
    const body = await screenBody();
    await within(body).findByRole("heading", { level: 1 });
    // Every read the gate draws on has landed: the MANIFEST, the listing and
    // the engrams beside them.
    await within(body).findByRole("article", { name: "MANIFEST" });
    await within(body).findByRole("link", { name: /Alpha/ });
    await waitFor(() => {
      expect(requested()).toContain("/domains");
    });
    await waitFor(() => {
      expect(
        within(body).queryByRole("link", { name: "Edit MANIFEST" }) !== null,
      ).toBe(offered);
    });
    expect(
      within(body).queryAllByRole("button", { name: /^Add / }).length > 0,
    ).toBe(offered);
    const menu = await openActions();
    expect(
      within(menu).getByRole("menuitem", { name: "Download archive" }),
    ).toBeInTheDocument();
    expect(
      within(menu).queryByRole("menuitem", { name: "Import archive" }) !== null,
    ).toBe(offered);
    const user = userEvent.setup();
    await user.keyboard("{Escape}");
    await user.keyboard("{Meta>}k{/Meta}");
    expect(
      await screen.findByRole("option", { name: /download archive/i }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: /edit manifest/i }) !== null,
    ).toBe(offered);
    expect(
      screen.queryByRole("option", { name: /import archive/i }) !== null,
    ).toBe(offered);
    await user.keyboard("{Escape}");
  }

  it("offers an admin no MANIFEST edit and no import on a locked domain, and keeps the policies link", async () => {
    serve(
      {
        "/domains": () => ({
          ...domainsResponse(),
          domains: [{ ...domainsResponse().domains[0], read_only: true }],
        }),
        "/domains/eng/manifest": lockedManifest,
      },
      "admin",
    );
    renderApp("/d/eng");
    await expectManifestWrites(false);
    // The policies, where a person switches the lock off, are one link away.
    expect(screen.getByRole("link", { name: /^Policies/ })).toHaveAttribute(
      "href",
      "/d/eng/settings#policies",
    );
  });

  it("offers an admin off the write list no MANIFEST edit and no import", async () => {
    serve(
      {
        "/domains": () => ({
          ...domainsResponse(),
          domains: [{ ...domainsResponse().domains[0], read_only: true }],
        }),
      },
      "admin",
    );
    renderApp("/d/eng");
    await expectManifestWrites(false);
  });

  it("offers an admin the MANIFEST edit and the import on a writable domain", async () => {
    serve({}, "admin");
    renderApp("/d/eng");
    await expectManifestWrites(true);
  });

  it("renders the MANIFEST as the page and the engrams at the root of the domain", async () => {
    serve();

    renderApp("/d/eng");

    expect(
      await screen.findByRole("heading", { level: 1, name: "eng" }),
    ).toBeVisible();
    // The MANIFEST's own `# eng` is the eyebrow over the page's heading, its
    // first paragraph the lead, and its sections the prose.
    const manifest = await screen.findByRole("article", { name: "MANIFEST" });
    expect(
      await within(manifest).findByText(
        "What this domain is for, in one paragraph.",
      ),
    ).toBeVisible();
    expect(
      within(manifest).getByRole("heading", { level: 2, name: "When to Use" }),
    ).toBeVisible();
    expect(screen.getAllByRole("heading", { name: "eng" })).toHaveLength(1);
    const row = await within(await screenBody()).findByRole("link", {
      name: /Alpha/,
    });
    expect(row).toHaveAttribute("href", "/d/eng/e/alpha");
    // The count stands beside the Engrams heading, the order menu on the row
    // above the list.
    const engrams = screen.getByRole("region", { name: "Engrams" });
    expect(await within(engrams).findByText("2")).toBeVisible();
    expect(
      within(engrams).getByRole("button", { name: "Order: Newest first" }),
    ).toBeVisible();
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

  it("lists the engrams as rows: title, folder, description, type and date", async () => {
    serve({
      "/domains/eng/engrams": (path) => {
        const page = engramsResponse(path) as {
          hits: Record<string, unknown>[];
        };
        return {
          ...page,
          hits: page.hits.map((hit) => ({
            ...hit,
            snippet: `About ${String(hit.title)}.`,
            recorded_at: "2026-09-12",
          })),
        };
      },
    });

    renderApp("/d/eng");

    const engrams = await screen.findByRole("region", { name: "Engrams" });
    const beta = await within(engrams).findByRole("link", {
      name: "Beta, notes/beta",
    });
    expect(within(beta).getByText("Beta")).toBeVisible();
    // The folder in mono beside the title, the description under it, the
    // type on the right.
    expect(within(beta).getByText("notes")).toHaveClass("font-mono");
    expect(within(beta).getByText("About Beta.")).toBeVisible();
    expect(within(beta).getByText("engram")).toBeVisible();
    // The recorded day, once the listing carries it.
    expect(within(beta).getByTitle("2026-09-12")).toHaveTextContent(
      /12 Sep 2026|Sep 12, 2026/,
    );
    // The folders as chips above the list, the domain's root as "All".
    const folders = within(engrams).getByRole("navigation", {
      name: "Folders",
    });
    expect(within(folders).getByText("All")).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(
      within(folders).getByRole("button", { name: "notes" }),
    ).toBeVisible();
  });

  it("keeps the Type and Status form behind Filter until it is asked for", async () => {
    serve();

    renderApp("/d/eng");

    const engrams = await screen.findByRole("region", { name: "Engrams" });
    const filter = await within(engrams).findByRole("button", {
      name: "Filter",
    });
    expect(filter).toHaveAttribute("aria-expanded", "false");
    expect(within(engrams).queryByLabelText("Type")).toBeNull();
    await userEvent.click(filter);
    expect(filter).toHaveAttribute("aria-expanded", "true");
    expect(within(engrams).getByLabelText("Type")).toBeVisible();
    expect(within(engrams).getByLabelText("Status")).toBeVisible();
  });

  it("opens the form for an address that already carries a filter", async () => {
    serve();

    renderApp("/d/eng?tags=eng");

    const engrams = await screen.findByRole("region", { name: "Engrams" });
    expect(
      await within(engrams).findByRole("button", { name: "Filter" }),
    ).toHaveAttribute("aria-expanded", "true");
    expect(within(engrams).getByLabelText("Type")).toBeVisible();
  });

  it("offers Edit MANIFEST to an admin", async () => {
    serve({}, "admin");

    renderApp("/d/eng");

    const body = await screenBody();
    expect(
      await within(body).findByRole("link", { name: "Edit MANIFEST" }),
    ).toHaveAttribute("href", "/d/eng/manifest/edit");
  });

  it("offers no Edit MANIFEST below admin", async () => {
    serve();

    renderApp("/d/eng");

    const body = await screenBody();
    await within(body).findByText("What this domain is for, in one paragraph.");
    expect(
      within(body).queryByRole("link", { name: "Edit MANIFEST" }),
    ).toBeNull();
  });

  /** A MANIFEST with every section, routing and configuration both. */
  const FULL_MANIFEST = [
    "---",
    "title: eng",
    "---",
    "",
    "# Engineering",
    "",
    "What this domain is for, in one paragraph.",
    "",
    "## Scope",
    "",
    "- Everything about eng",
    "",
    "## When to Use",
    "",
    "- Route here for eng questions.",
    "",
    "## Notes for Agents",
    "",
    "- Write one engram per decision.",
    "",
    "## Provisioning",
    "",
    "Installed by the setup script.",
    "",
    "- skills: skills",
    "- agents: ../agents",
    "",
    "## Tag Aliases",
    "",
    "- Multi_Word -> multi-word",
    "",
  ].join("\n");

  function fullManifest(overrides: Record<string, unknown> = {}) {
    return () => ({
      domain: "eng",
      markdown: FULL_MANIFEST,
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
        ...overrides,
      }),
    });
  }

  it("draws the MANIFEST once: the prose in the page, the configuration only in the rail", async () => {
    serve({ "/domains/eng/manifest": fullManifest() });

    renderApp("/d/eng");

    const manifest = await screen.findByRole("article", { name: "MANIFEST" });
    // The eyebrow is the MANIFEST's own title, over the domain's name.
    expect(
      await within(await screenBody()).findByText("Engineering"),
    ).toBeVisible();
    // The prose: When to Use first, with the line that says what agents do
    // with it, then the rest in file order. Scope is drawn once.
    const headings = within(manifest)
      .getAllByRole("heading", { level: 2 })
      .map((h) => h.textContent);
    expect(headings).toEqual(["When to Use", "Scope", "Notes for Agents"]);
    expect(
      within(manifest).getByText(
        "Agents read this to decide when to use this domain.",
      ),
    ).toBeVisible();
    expect(screen.queryByText("Agents route by When to Use.")).toBeNull();
    expect(await screen.findAllByText("Everything about eng")).toHaveLength(1);
    expect(
      await within(manifest).findByText("Write one engram per decision."),
    ).toBeVisible();
    // No Manifest cards panel, no Routing card.
    expect(screen.queryByRole("region", { name: "Routing" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Manifest" })).toBeNull();

    // The configuration sections are cut from the prose and shown once, in
    // the rail, from what the core crate parsed out of them.
    expect(within(manifest).queryByText(/skills/)).toBeNull();
    expect(within(manifest).queryByText(/Multi_Word/)).toBeNull();
    const rail = screen.getByRole("complementary", {
      name: "About this domain",
    });
    const provisioning = within(rail).getByRole("region", {
      name: "Provisioning",
    });
    expect(within(provisioning).getAllByText("skills")).toHaveLength(2);
    expect(within(provisioning).getByText("agents")).toBeVisible();
    // What the section says beside its declarations is shown with them.
    expect(
      await within(provisioning).findByText("Installed by the setup script."),
    ).toBeVisible();
    expect(
      within(manifest).queryByText("Installed by the setup script."),
    ).toBeNull();
    expect(within(provisioning).getByText("../agents")).toBeVisible();
    const aliases = within(rail).getByRole("region", { name: "Tag Aliases" });
    expect(within(aliases).getByText(/Multi_Word/)).toBeVisible();
    expect(within(aliases).getByText(/multi-word/)).toBeVisible();
  });

  it("cuts a configuration section the registry adds later without a change here", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: `${MANIFEST}\n## Future Switches\n\n- something: on\n`,
        sections: sectionsResponse({
          starters: [
            ...defaultStarterRows(),
            { section: "Future Switches", meaning: "", example: "" },
          ],
        }),
      }),
    });

    renderApp("/d/eng");

    const manifest = await screen.findByRole("article", { name: "MANIFEST" });
    await within(manifest).findByText("Route here for eng questions.");
    expect(within(manifest).queryByText("something: on")).toBeNull();
    const rail = screen.getByRole("complementary", {
      name: "About this domain",
    });
    const block = within(rail).getByRole("region", {
      name: "Future Switches",
    });
    expect(await within(block).findByText("something: on")).toBeVisible();
  });

  it("fills the rail: about and the settings with their values, and nothing the list already says", async () => {
    const synced = new Date(Date.now() - 2 * 3600 * 1000).toISOString();
    serve(
      {
        "/domains": () => ({
          ...domainsResponse(),
          domains: [{ ...domainsResponse().domains[0], last_sync: synced }],
        }),
        "/domains/eng/manifest": fullManifest(),
      },
      "admin",
    );

    renderApp("/d/eng");

    const rail = await screen.findByRole("complementary", {
      name: "About this domain",
    });
    const about = await within(rail).findByRole("region", { name: "About" });
    expect(await within(about).findByText("4 engrams")).toBeVisible();
    expect(within(about).getByText("Shared with everyone")).toBeVisible();
    expect(within(about).getByText("Files on disk")).toBeVisible();
    // When it last synced in a reader's words, the instant in the title.
    const when = within(about).getByText("2 hours ago");
    expect(when).toHaveAttribute("title", expect.stringMatching(/^\d{4}-/));

    const settings = within(rail).getByRole("region", { name: "Settings" });
    expect(
      await within(settings).findByRole("link", { name: /^Members everyone/ }),
    ).toHaveAttribute("href", "/d/eng/settings#members");
    expect(
      within(settings).getByRole("link", { name: /^Policies 0 set/ }),
    ).toHaveAttribute("href", "/d/eng/settings#policies");
    expect(
      within(settings).getByRole("link", { name: /^Review mode off/ }),
    ).toHaveAttribute("href", "/d/eng/settings#review-mode");
    expect(
      within(settings).getByRole("link", { name: "Backup and import" }),
    ).toHaveAttribute("href", "/d/eng/settings#backup");

    // What was recorded lately is the list below, newest first: the rail
    // does not say it a second time.
    expect(
      within(rail).queryByRole("region", { name: "Recently recorded" }),
    ).toBeNull();
    expect(requested()).not.toContain("/activity");
  });

  it("draws no eyebrow over a MANIFEST whose title is the domain's own name", async () => {
    serve();

    renderApp("/d/eng");

    // The scaffold every new domain starts from is \`# <name>\`, which
    // would only say the heading twice.
    const manifest = await screen.findByRole("article", { name: "MANIFEST" });
    await within(manifest).findByText(
      "What this domain is for, in one paragraph.",
    );
    const heading = screen.getByRole("heading", { level: 1, name: "eng" });
    const header = heading.closest("header") as HTMLElement;
    expect(within(header).getAllByText("eng", { exact: true })).toEqual([
      heading,
    ]);
  });

  it("reads a MANIFEST written with CRLF line endings", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: FULL_MANIFEST.replace(/\n/g, "\r\n"),
        sections: fullManifest()().sections,
      }),
    });

    renderApp("/d/eng");

    const manifest = await screen.findByRole("article", { name: "MANIFEST" });
    expect(
      await within(await screenBody()).findByText("Engineering"),
    ).toBeVisible();
    expect(
      within(manifest)
        .getAllByRole("heading", { level: 2 })
        .map((h) => h.textContent),
    ).toEqual(["When to Use", "Scope", "Notes for Agents"]);
    expect(within(manifest).queryByText(/Multi_Word/)).toBeNull();
  });

  it("gives each section heading the id the document renderer would, and draws its markdown", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown:
          "# eng\n\nLead.\n\n## When to Use\n\n- a\n\n## The `deploy` flow\n\nText.\n\n## Rollout :status[Beta]{blue}\n\nSoon.\n",
        sections: sectionsResponse(),
      }),
    });

    renderApp("/d/eng");

    const manifest = await screen.findByRole("article", { name: "MANIFEST" });
    const flow = await within(manifest).findByRole("heading", {
      level: 2,
      name: "The deploy flow",
    });
    expect(flow).toHaveAttribute("id", "the-deploy-flow");
    expect(within(flow).getByText("deploy").tagName).toBe("CODE");
    // A status label in a heading reads as its words, in the slug too.
    expect(
      within(manifest).getByRole("heading", { level: 2, name: /^Rollout/ }),
    ).toHaveAttribute("id", "rollout-beta");
    expect(
      within(manifest).getByRole("heading", { level: 2, name: "When to Use" }),
    ).toHaveAttribute("id", "when-to-use");
  });

  /**
   * The MANIFEST as the server lists it: a row in the root tree whose file is
   * the root MANIFEST.md, under a permalink of its own choosing, and a hit
   * in every listing that is not filtered away from it.
   */
  function withManifestRow(): Record<string, Answer> {
    const manifestHit = {
      domain: "eng",
      permalink: "about-eng",
      title: "MANIFEST",
      engram_type: "manifest",
      kind: "engram",
      status: "current",
      tags: [],
    };
    return {
      "/domains/eng/tree": (path) => {
        const tree = treeResponse(path) as {
          engrams: Record<string, unknown>[];
        };
        return path.includes("path=")
          ? tree
          : {
              ...tree,
              engrams: [
                {
                  permalink: "about-eng",
                  title: "MANIFEST",
                  type: "manifest",
                  path: "MANIFEST.md",
                },
                ...tree.engrams,
              ],
            };
      },
      "/domains/eng/engrams": (path) => {
        const page = engramsResponse(path) as {
          total: number;
          count: number;
          hits: Record<string, unknown>[];
        };
        if (path.includes("type=manifest")) {
          return { ...page, total: 1, count: 1, hits: [manifestHit] };
        }
        return path.includes("tags=")
          ? page
          : {
              ...page,
              total: page.total + 1,
              count: page.count + 1,
              hits: [manifestHit, ...page.hits],
            };
      },
    };
  }

  it("does not list the MANIFEST under itself, and no count includes it", async () => {
    serve(withManifestRow());

    renderApp("/d/eng");

    const engrams = await screen.findByRole("region", { name: "Engrams" });
    await within(engrams).findByRole("link", { name: /Alpha/ });
    expect(
      within(engrams).queryByRole("link", { name: /MANIFEST/ }),
    ).toBeNull();
    // The heading and About agree: 2 of the listing's 3 and 3 of the
    // summary's 4, the MANIFEST left out of both.
    expect(await within(engrams).findByText("2")).toBeVisible();
    const about = await screen.findByRole("region", { name: "About" });
    expect(await within(about).findByText("3 engrams")).toBeVisible();
  });

  it("still finds the MANIFEST through an explicit type filter", async () => {
    serve(withManifestRow());

    renderApp("/d/eng?type=manifest");

    const engrams = await screen.findByRole("region", { name: "Engrams" });
    expect(
      await within(engrams).findByRole("link", { name: /^MANIFEST, / }),
    ).toHaveAttribute("href", "/d/eng/e/about-eng");
  });

  it("reads prose, then the engrams, then the rail, in document order", async () => {
    serve();

    renderApp("/d/eng");

    const manifest = await screen.findByRole("article", { name: "MANIFEST" });
    const engrams = await screen.findByRole("region", { name: "Engrams" });
    const rail = await screen.findByRole("complementary", {
      name: "About this domain",
    });
    expect(
      manifest.compareDocumentPosition(engrams) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      engrams.compareDocumentPosition(rail) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("keeps the backup link and the review mode row from a caller who would be refused there", async () => {
    serve();

    renderApp("/d/eng");

    const rail = await screen.findByRole("complementary", {
      name: "About this domain",
    });
    const settings = within(rail).getByRole("region", { name: "Settings" });
    await within(settings).findByRole("link", { name: /^Members/ });
    expect(
      within(settings).queryByRole("link", { name: "Backup and import" }),
    ).toBeNull();
  });

  it("offers each action in the More actions menu by right", async () => {
    serve({}, "admin");
    renderApp("/d/eng");
    const admin = await openActions();
    expect(
      within(admin)
        .getAllByRole("menuitem")
        .map((item) => item.textContent),
    ).toEqual([
      "Rename domain",
      "Download archive",
      "Import archive",
      "Domain settings",
    ]);
    expect(
      within(admin).getByRole("menuitem", { name: "Download archive" }),
    ).toHaveAttribute("href", "/api/v1/domains/eng/archive");
    expect(
      within(admin).getByRole("menuitem", { name: "Domain settings" }),
    ).toHaveAttribute("href", "/d/eng/settings");
  });

  it("offers an editor only the way to the domain's settings", async () => {
    serve();
    renderApp("/d/eng");
    const menu = await openActions();
    expect(
      within(menu)
        .getAllByRole("menuitem")
        .map((item) => item.textContent),
    ).toEqual(["Domain settings"]);
  });

  it("draws a routing bullet's markdown instead of its punctuation", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown:
          "# eng\n\nLead.\n\n## When to Use\n\n- **Search `project2030` first**\n",
        sections: sectionsResponse({
          when_to_use: ["**Search `project2030` first**"],
        }),
      }),
    });

    renderApp("/d/eng");

    const manifest = await screen.findByRole("article", { name: "MANIFEST" });
    const code = await within(manifest).findByText("project2030");
    expect(code.tagName).toBe("CODE");
    expect(code.closest("strong")).not.toBeNull();
    expect(within(manifest).queryByText(/\*\*/)).toBeNull();
  });

  it("names a routing section a MANIFEST lacks, and how agents route then", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: "# eng\n\nLead.\n\n## Scope\n\n- Everything about eng\n",
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
    // words, and the syntax that starts it.
    const manifest = await screen.findByRole("article", { name: "MANIFEST" });
    expect(
      await within(manifest).findByText(
        /Agents pick this domain by these bullets/,
      ),
    ).toBeVisible();
    expect(
      within(manifest).getByText(
        "Agents read Scope to decide when to use this domain, because When to Use is empty.",
      ),
    ).toBeVisible();
    expect(
      within(manifest).queryByText(
        /What belongs in this domain and what does not/,
      ),
    ).toBeNull();
  });

  it("says when no agent can route here", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: "# eng\n\nLead.\n",
        sections: sectionsResponse({
          when_to_use: [],
          routing: "none",
          missing: ["Scope", "When to Use"],
        }),
      }),
    });

    renderApp("/d/eng");

    const manifest = await screen.findByRole("article", { name: "MANIFEST" });
    expect(
      await within(manifest).findByText(
        /What belongs in this domain and what does not/,
      ),
    ).toBeVisible();
    expect(
      within(manifest).getByText(/Agents pick this domain by these bullets/),
    ).toBeVisible();
    expect(
      within(manifest).getByText(
        "Agents cannot route here until When to Use has a bullet.",
      ),
    ).toBeVisible();
  });

  it("lists every problem bullet with its reason, in the rail", async () => {
    serve({
      "/domains/eng/manifest": fullManifest({
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
      }),
    });

    renderApp("/d/eng");

    const rail = await screen.findByRole("complementary", {
      name: "About this domain",
    });
    const provisioning = await within(rail).findByRole("region", {
      name: "Provisioning",
    });
    expect(within(provisioning).getByText("widgets: w")).toBeVisible();
    expect(
      within(provisioning).getByText(
        "unknown provisioning type `widgets`, expected one of skills, commands, agents or mcps",
      ),
    ).toBeVisible();
  });

  it("draws the document whole for a server that sends no sections", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: `${MANIFEST}\n## Tag Aliases\n\n- k8s -> kubernetes\n`,
      }),
    });

    renderApp("/d/eng");

    // An older daemon answers the markdown alone. The document is still
    // read, and nothing is cut, because nothing says what is configuration.
    const manifest = await screen.findByRole("article", { name: "MANIFEST" });
    expect(
      await within(manifest).findByText(
        "What this domain is for, in one paragraph.",
      ),
    ).toBeVisible();
    expect(
      await within(manifest).findByText("k8s -> kubernetes"),
    ).toBeVisible();
    expect(screen.queryByRole("link", { name: /^Policies/ })).toBeNull();
  });

  it("shows the explanation but no action to a reader who cannot edit", async () => {
    serve();

    renderApp("/d/eng");

    // The honest answer for somebody without the right: they can still learn
    // what the missing section does, they just cannot start it from here.
    const manifest = await screen.findByRole("article", { name: "MANIFEST" });
    expect(
      await within(manifest).findByText(
        /What belongs in this domain and what does not/,
      ),
    ).toBeVisible();
    expect(
      within(manifest).queryByRole("button", { name: "Add Scope" }),
    ).toBeNull();
  });

  it("offers a blank MANIFEST's routing sections and the whole document to start", async () => {
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

    expect(await screen.findByText(/no MANIFEST yet/)).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Add When to Use" }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "Add Scope" })).toBeVisible();
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

    // The document itself is still drawn, because repairing it is what
    // somebody does next.
    expect(
      await screen.findByText("What this domain is for, in one paragraph."),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Create a MANIFEST" }),
    ).toBeNull();
  });

  it("carries the section to the editor when the action is pressed", async () => {
    serve({ "/validate": () => ({ findings: [], errors: 0 }) }, "admin");

    renderApp("/d/eng");

    await userEvent.click(
      await screen.findByRole("button", { name: "Add Scope" }),
    );

    // The whole hop, end to end: the button names a section, the navigation
    // state carries the name, and the editor puts the server's own example
    // for it into the buffer.
    expect(
      await screen.findByRole("heading", { name: "Editing eng MANIFEST" }),
    ).toBeVisible();
    const editor = await screen.findByLabelText("MANIFEST source");
    await waitFor(() => {
      expect(editor.textContent).toContain("## Scope");
    });
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

  it("shows an editor with no share role her own draft conflict, outside the team banners", async () => {
    serve({
      "/domains": () => ({
        behavior: [],
        domains: [
          {
            name: "eng",
            kind: "file",
            engrams: 4,
            review: "overlay",
            my_drafts: 1,
            when_to_use: ["Route here for eng questions."],
          },
        ],
      }),
      "/domains/eng/drafts/conflicts": () => ({
        conflicts: [
          {
            id: "d1234567",
            path: "notes/a.md",
            kind: "EditEdit",
            moved_from: null,
            other: null,
            file: false,
            draft: true,
          },
        ],
        merged: [],
      }),
    });

    renderApp("/d/eng");

    const notice = await screen.findByRole("region", { name: "Your drafts" });
    expect(
      within(notice).getByRole("button", { name: "notes/a.md" }),
    ).toBeVisible();
    // She may not share, so no sync card, and the status it reads is never
    // asked for: the notice is her warning.
    expect(screen.queryByRole("region", { name: "Team sync" })).toBeNull();
    expect(requested()).not.toContain("/domains/eng/sync");
  });

  it("lists a draft conflict once, in the notice, beside the sync card", async () => {
    // An admin, so the sync card draws too. The status lists her draft
    // conflict after the folder's (it has none here); the card leaves it to
    // the notice, which opens it through the draft routes.
    const entry = {
      id: "d1234567",
      path: "notes/a.md",
      kind: "EditEdit",
      moved_from: null,
      other: null,
      file: false,
      draft: true,
    };
    serve(
      {
        "/domains/eng/sync": () => syncResponse({ conflicts: [entry] }),
        "/domains/eng/drafts/conflicts": () => ({
          conflicts: [entry],
          merged: [],
        }),
      },
      "admin",
    );

    renderApp("/d/eng");

    const card = await screen.findByRole("region", { name: "Team sync" });
    const notice = await screen.findByRole("region", { name: "Your drafts" });
    const buttons = screen.getAllByRole("button", { name: "notes/a.md" });
    expect(buttons).toHaveLength(1);
    expect(notice).toContainElement(buttons[0] ?? null);
    expect(within(card).queryByText(/conflicts? to settle/)).toBeNull();
  });

  it("shows her draft conflict on a read-only domain, with what is left to her", async () => {
    serve({
      "/domains": () => ({
        ...domainsResponse(),
        domains: [{ ...domainsResponse().domains[0], read_only: true }],
      }),
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: "# eng",
        sections: manifestSectionsResponse({
          policies: defaultPolicyRows().map((row) =>
            row.key === "read_only"
              ? { ...row, declared: "on", effective: "on" }
              : row,
          ),
        }),
      }),
      "/domains/eng/drafts/conflicts": () => ({
        conflicts: [
          {
            id: "d1234567",
            path: "notes/a.md",
            kind: "EditEdit",
            moved_from: null,
            other: null,
            file: false,
            draft: true,
          },
        ],
        merged: [],
      }),
    });

    renderApp("/d/eng");

    const notice = await screen.findByRole("region", { name: "Your drafts" });
    expect(notice).toHaveTextContent(
      "This domain is read-only. Until its owner switches read_only off, you can only take the team's text for these drafts.",
    );
  });

  it("shows her draft conflict while a fold that stopped part way is unfinished", async () => {
    // The listing says the domain takes changes directly - the fold took the
    // review key off - but her drafts still stand and must be settled, and
    // the list route still serves them.
    serve({
      "/domains/eng/drafts/conflicts": () => ({
        conflicts: [
          {
            id: "d1234567",
            path: "notes/a.md",
            kind: "EditEdit",
            moved_from: null,
            other: null,
            file: false,
            draft: true,
          },
        ],
        merged: [],
      }),
    });

    renderApp("/d/eng");

    const notice = await screen.findByRole("region", { name: "Your drafts" });
    expect(
      within(notice).getByRole("button", { name: "notes/a.md" }),
    ).toBeVisible();
  });

  it("shows nothing about draft conflicts on a domain that takes changes directly", async () => {
    serve({
      "/domains/eng/drafts/conflicts": () => ({ conflicts: [], merged: [] }),
    });

    renderApp("/d/eng");

    expect(await screen.findByRole("heading", { name: "eng" })).toBeVisible();
    await waitFor(() => {
      expect(requested()).toContain("/domains/eng/drafts/conflicts");
    });
    expect(screen.queryByRole("region", { name: "Your drafts" })).toBeNull();
  });

  it("asks nothing about draft conflicts for a session that may not write", async () => {
    serve({}, "editor", () =>
      meResponse({ user: userFixture({ role: "viewer" }) }),
    );

    renderApp("/d/eng");

    expect(await screen.findByRole("heading", { name: "eng" })).toBeVisible();
    expect(requested().some((path) => path.includes("/drafts/conflicts"))).toBe(
      false,
    );
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
    await screen.findByRole("article", { name: "MANIFEST" });
    await within(await screenBody()).findByRole("link", { name: /Alpha/ });
    expect(screen.queryByText(/no MANIFEST yet/)).toBeNull();
  });

  it("launches a new engram from the heading of the list it lands in", async () => {
    serve();

    renderApp("/d/eng");
    const body = await screenBody();

    // The secondary tier (spec 3.1): the filled primary is kept for the step
    // that commits a flow, and the sidebar's launcher never co-renders with
    // it.
    const launcher = await within(body).findByRole("button", {
      name: "New engram",
    });
    expect(launcher.className).toContain("border-slate-300");
    expect(launcher.className).not.toContain("bg-accent-700");
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

  it("keeps the editor link for an admin when an older daemon answers 404", async () => {
    serve(
      {
        "/domains/eng/manifest": () => {
          throw new ApiProblem(404, "not found", "no MANIFEST in domain 'eng'");
        },
      },
      "admin",
    );

    renderApp("/d/eng");

    // The daemon predates `missing: true`, so the gap arrives as a 404. That
    // reader still gets the sentence and the way to the editor; the starter
    // button needs the registry the 404 did not carry, so it is not drawn.
    const body = await screenBody();
    const section = await screen.findByRole("article", { name: "MANIFEST" });
    expect(await within(section).findByText(/no MANIFEST yet/)).toBeVisible();
    expect(
      within(body).getByRole("link", { name: "Edit MANIFEST" }),
    ).toHaveAttribute("href", "/d/eng/manifest/edit");
    expect(
      within(body).queryByRole("button", { name: "Create a MANIFEST" }),
    ).toBeNull();
    expect(within(section).queryByRole("alert")).toBeNull();
  });

  it("offers an admin the editor over a manifest that is not there yet", async () => {
    const empty =
      "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const missing = {
      domain: "eng",
      markdown: "",
      checksum: empty,
      missing: true,
      sections: sectionsResponse({
        scope: [],
        when_to_use: [],
        routing: "none",
        missing: ["Scope", "When to Use"],
        policies: [],
      }),
    };
    /** The markdown one request carried, as the app serialized it. */
    const sentMarkdown = (init?: RequestInit): string =>
      (
        JSON.parse(typeof init?.body === "string" ? init.body : "{}") as {
          markdown?: string;
        }
      ).markdown ?? "";
    const put = vi.fn((_path: string, init?: RequestInit) => ({
      ...missing,
      markdown: sentMarkdown(init),
      checksum: "m1",
      missing: false,
    }));
    serve(
      {
        "/domains/eng/manifest": (path, init) =>
          init?.method === "PUT" ? put(path, init) : missing,
        "/validate": () => ({ findings: [], errors: 0 }),
      },
      "admin",
    );

    renderApp("/d/eng");

    // A domain with no MANIFEST is exactly the domain an admin opens the
    // editor to fix, so the gap sentence comes with the way to close it.
    const body = await screenBody();
    const section = await screen.findByRole("article", { name: "MANIFEST" });
    await within(section).findByText(/no MANIFEST yet/);
    expect(
      within(body).getByRole("link", { name: "Edit MANIFEST" }),
    ).toHaveAttribute("href", "/d/eng/manifest/edit");

    // And the way through: the seed, then a save that creates the file under
    // the checksum the read handed out for nothing.
    await userEvent.click(
      within(section).getByRole("button", { name: "Create a MANIFEST" }),
    );
    const editor = await screen.findByLabelText("MANIFEST source");
    await waitFor(() => {
      expect(editor.textContent).toContain("## When to Use");
    });
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(put).toHaveBeenCalled();
    });
    const init = put.mock.calls[0]?.[1];
    expect((init?.headers as Record<string, string>)["If-Match"]).toBe(
      `"${empty}"`,
    );
    expect(sentMarkdown(init)).toMatch(/^---\n/);
    expect(await screen.findByText("Saved")).toBeInTheDocument();
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
      "/d/eng/engrams",
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
    // The team cards are lazy on the domain page, so an absent card could
    // only mean its chunk had not landed yet. No loading shape either says
    // the folder page never asked for them.
    expect(
      within(body).queryByRole("status", { name: "Loading the members" }),
    ).toBeNull();
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
    ).toHaveAttribute("href", "/d/eng/engrams?path=notes");
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

    // The tags wait behind Filter with the rest of the frontmatter form.
    await userEvent.click(within(body).getByRole("button", { name: "Filter" }));
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

    // The tags wait behind Filter with the rest of the frontmatter form.
    await userEvent.click(within(body).getByRole("button", { name: "Filter" }));
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
  it("folds the team banners behind one line on a phone", async () => {
    serve({ "/domains/eng/sync": () => syncResponse() }, "admin");

    renderApp("/d/eng");
    const body = await screenBody();

    // The line says what waits; the cards behind it open on a press. From
    // md up the line is hidden and the cards always show (CSS, not jsdom's).
    const line = await within(body).findByRole("button", {
      name: "Team sync · 2 local changes, 1 open proposal",
    });
    expect(line).toHaveAttribute("aria-expanded", "false");
    const panel = document.getElementById(
      line.getAttribute("aria-controls") ?? "",
    ) as HTMLElement;
    expect(panel.className).toMatch(/(^| )hidden( |$)/);
    expect(panel.className).toContain("md:flex");
    await userEvent.click(line);
    expect(line).toHaveAttribute("aria-expanded", "true");
    expect(panel.className).not.toMatch(/(^| )hidden( |$)/);
  });

  /** The phone line and the panel behind it, once the status has landed. */
  async function phoneBanners(): Promise<{
    line: HTMLElement;
    panel: HTMLElement;
  }> {
    const body = await screenBody();
    const card = await within(body).findByRole("region", { name: "Team sync" });
    const panel = card.parentElement as HTMLElement;
    const line = await within(body).findByText("Team sync", {
      selector: "span",
    });
    return { line: line.parentElement as HTMLElement, panel };
  }

  /** Whether the panel is folded away below md. */
  function folded(panel: HTMLElement): boolean {
    return /(^| )hidden( |$)/.test(panel.className);
  }

  for (const [label, overrides] of [
    ["a failed origin check", { probe_error: "GitHub did not answer" }],
    [
      "no connection",
      { connection: { connected: false, user: null, token_store: null } },
    ],
    ["conflicts", { conflicts: ["notes/a.md"] }],
    [
      "declined proposals",
      { declined_proposals: [{ number: 3 }, { number: 4 }] },
    ],
  ] as const) {
    it(`keeps the team banners open on a phone for ${label}, without a summary line above the card`, async () => {
      serve(
        { "/domains/eng/sync": () => syncResponse({ ...overrides }) },
        "admin",
      );

      renderApp("/d/eng");

      const body = await screenBody();
      const card = await within(body).findByRole("region", {
        name: "Team sync",
      });
      expect(folded(card.parentElement as HTMLElement)).toBe(false);
      expect(
        within(body).queryByText("Team sync", { selector: "span" }),
      ).toBeNull();
    });
  }

  it("keeps the team banners open on a phone when the status read is refused", async () => {
    serve(
      {
        "/domains/eng/sync": () => {
          throw new ApiProblem(502, "bad gateway", "the origin did not answer");
        },
      },
      "admin",
    );

    renderApp("/d/eng");

    // Nothing to fold behind: the card stands open with the refusal in it.
    const body = await screenBody();
    const card = await within(body).findByRole("region", { name: "Team sync" });
    expect(within(card).getByRole("alert")).toBeVisible();
    expect(folded(card.parentElement as HTMLElement)).toBe(false);
    expect(
      within(body).queryByRole("button", { name: /^Team sync ·/ }),
    ).toBeNull();
  });

  it("unfolds the team banners on a phone once Sync now fails", async () => {
    serve(
      {
        "/domains/eng/sync": (_path, init) => {
          if (init?.method === "POST") {
            throw new ApiProblem(422, "refused", "the pull was refused");
          }
          return syncResponse();
        },
      },
      "admin",
    );

    renderApp("/d/eng");

    const { line, panel } = await phoneBanners();
    expect(folded(panel)).toBe(true);
    await userEvent.click(line);
    await userEvent.click(
      within(panel).getByRole("button", { name: "Sync now" }),
    );
    await waitFor(() => {
      expect(within(panel).getByText(/the pull was refused/)).toHaveAttribute(
        "role",
        "alert",
      );
    });
    // Folding is no longer offered while the failure stands.
    expect(screen.queryByRole("button", { name: /^Team sync ·/ })).toBeNull();
    expect(folded(panel)).toBe(false);
  });

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
 * name fields: `canonical_name`, `aliases`, `name_origin`,
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
    const menu = await openActions();
    expect(
      within(menu).getByRole("menuitem", { name: "Rename domain" }),
    ).toBeVisible();
  });

  it("offers an admin to finish or keep a rename the MANIFEST made, and each answer closes the question", async () => {
    let open = true;
    const calls: string[] = [];
    serve(
      {
        "/domains": () =>
          listingOf(
            domainRow({
              canonical_name: "platform",
              rename_pending: {
                from: "eng",
                to: "platform",
                seen_at: "2026-10-20T10:00:00Z",
              },
            }),
          ),
        "/domains/eng/rename/finish": (_path, init) => {
          calls.push(`finish ${init?.method}`);
          open = false;
          return {
            domain: "eng",
            from: "eng",
            to: "platform",
            rewritten: [{ domain: "ops", engrams: 1, references: 2 }],
            left_behind: [],
          };
        },
        "/domains/eng/rename/keep": (_path, init) => {
          calls.push(`keep ${init?.method}`);
          open = false;
          return {
            domain: "eng",
            from: "eng",
            to: "platform",
            answered: "keep",
          };
        },
      },
      "admin",
    );
    // The listing answers the question as open until a call closes it.
    const listing = apiMock.getMockImplementation();
    apiMock.mockImplementation(async (path: string, init?: RequestInit) => {
      const answer = (await (listing as NonNullable<typeof listing>)(
        path,
        init,
      )) as { domains?: Record<string, unknown>[] };
      if (path === "/domains" && !open && answer.domains) {
        return {
          ...answer,
          domains: answer.domains.map((row) => ({
            ...row,
            rename_pending: null,
          })),
        };
      }
      return answer;
    });

    renderApp("/d/eng");

    const sentence = await screen.findByText(
      "This domain was renamed from eng to platform in its MANIFEST.",
    );
    const banner = sentence.closest('[role="status"]') as HTMLElement;
    expect(
      within(banner).getByRole("button", { name: "Keep links as they are" }),
    ).toBeVisible();
    await userEvent.click(
      within(banner).getByRole("button", { name: "Finish the rename" }),
    );

    await waitFor(() => {
      expect(screen.queryByText(/^This domain was renamed from/)).toBeNull();
    });
    expect(calls).toEqual(["finish POST"]);
    expect(
      await screen.findByText("Links now use platform instead of eng."),
    ).toBeVisible();
    expect(screen.getByText(/ops: 1 engram rewritten/)).toBeVisible();
  });

  it("keeps the links when asked to", async () => {
    serve(
      {
        "/domains": () =>
          listingOf(
            domainRow({
              rename_pending: {
                from: "eng",
                to: "platform",
                seen_at: "2026-10-20T10:00:00Z",
              },
            }),
          ),
        "/domains/eng/rename/keep": () => ({ answered: "keep" }),
      },
      "admin",
    );

    renderApp("/d/eng");

    await userEvent.click(
      await screen.findByRole("button", { name: "Keep links as they are" }),
    );

    await waitFor(() => {
      expect(requested()).toContain("/domains/eng/rename/keep");
    });
  });

  it("says in words when someone changed the name again", async () => {
    serve(
      {
        "/domains": () =>
          listingOf(
            domainRow({
              rename_pending: {
                from: "eng",
                to: "platform",
                seen_at: "2026-10-20T10:00:00Z",
              },
            }),
          ),
        "/domains/eng/rename/keep": () => {
          throw new ApiProblem(409, "conflict", "no rename is waiting");
        },
      },
      "admin",
    );

    renderApp("/d/eng");

    await userEvent.click(
      await screen.findByRole("button", { name: "Keep links as they are" }),
    );

    expect(
      await screen.findByText(
        "Someone changed the name again. Reload to see the new question.",
      ),
    ).toHaveAttribute("role", "alert");
  });

  it("says in words when finishing the rename fails", async () => {
    serve(
      {
        "/domains": () =>
          listingOf(
            domainRow({
              rename_pending: {
                from: "eng",
                to: "platform",
                seen_at: "2026-10-20T10:00:00Z",
              },
            }),
          ),
        "/domains/eng/rename/finish": () => {
          throw new ApiProblem(500, "failed", "the rewrite could not start");
        },
      },
      "admin",
    );

    renderApp("/d/eng");

    await userEvent.click(
      await screen.findByRole("button", { name: "Finish the rename" }),
    );

    expect(
      await screen.findByText("the rewrite could not start"),
    ).toHaveAttribute("role", "alert");
    expect(screen.getByText(/^This domain was renamed from/)).toBeVisible();
  });

  it("says no links needed a change when nothing was rewritten", async () => {
    serve(
      {
        "/domains": () =>
          listingOf(
            domainRow({
              rename_pending: {
                from: "eng",
                to: "platform",
                seen_at: "2026-10-20T10:00:00Z",
              },
            }),
          ),
        "/domains/eng/rename/finish": () => ({
          domain: "eng",
          from: "eng",
          to: "platform",
          rewritten: [],
          left_behind: [],
        }),
      },
      "admin",
    );

    renderApp("/d/eng");

    await userEvent.click(
      await screen.findByRole("button", { name: "Finish the rename" }),
    );

    expect(await screen.findByText("No links needed a change.")).toBeVisible();
  });

  it("shows the pending rename to nobody who may not rename the domain", async () => {
    serve({
      "/domains": () =>
        listingOf(
          domainRow({
            rename_pending: {
              from: "eng",
              to: "platform",
              seen_at: "2026-10-20T10:00:00Z",
            },
          }),
        ),
    });

    renderApp("/d/eng");

    await screen.findByRole("heading", { level: 1, name: "eng" });
    expect(screen.queryByText(/^This domain was renamed from/)).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Finish the rename" }),
    ).toBeNull();
  });

  it("keeps Rename domain inert on a read-only instance, and says why", async () => {
    serve({}, "admin", () =>
      meResponse({
        user: userFixture({ role: "admin" }),
        read_only: true,
      }),
    );

    renderApp("/d/eng");

    const menu = await openActions();
    const trigger = within(menu).getByRole("menuitem", {
      name: "Rename domain",
    });
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
    expect(screen.queryByRole("button", { name: "Rename" })).toBeNull();
    const menu = await openActions();
    expect(
      within(menu).queryByRole("menuitem", { name: "Rename domain" }),
    ).toBeNull();
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

    const editManifest = await screen.findByRole("link", {
      name: "Edit MANIFEST",
    });
    expect(editManifest).toHaveAttribute("aria-disabled", "true");
    expect(editManifest).toHaveAccessibleDescription(REASON);

    // A click on any of them does nothing: guarded at the press, not merely
    // painted as inert. New engram opens a dialog when it acts and Edit
    // MANIFEST navigates to the editor; neither does.
    await userEvent.click(newEngram);
    await userEvent.click(editManifest);
    expect(screen.queryByRole("dialog")).toBeNull();

    // The launcher and the import, rows of the More actions menu: the same
    // reason as every other control here, not the read-only one - a rename
    // in progress is what is shutting them. Unregister and the visibility
    // switch live on the settings screen, under the same rule.
    const menu = await openActions();
    const renameLauncher = within(menu).getByRole("menuitem", {
      name: "Rename domain",
    });
    expect(renameLauncher).toHaveAttribute("aria-disabled", "true");
    expect(renameLauncher).toHaveAccessibleDescription(REASON);
    const importArchive = within(menu).getByRole("menuitem", {
      name: "Import archive",
    });
    expect(importArchive).toHaveAttribute("aria-disabled", "true");
    expect(importArchive).toHaveAccessibleDescription(REASON);
    await userEvent.click(renameLauncher);
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

    // The members read is what says who owns the domain, so the row arrives
    // with it.
    await screen.findByRole("link", { name: "Members 0 members" });
    const menu = await openActions();
    expect(
      within(menu).getByRole("menuitem", { name: "Rename domain" }),
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
      // A query on the domain's address means the engram view, which has
      // its own address since 0.28.1; the rename redirect keeps that tail.
      expect(screen.getByTestId("probe")).toHaveTextContent(
        "/d/eng-knowledge/engrams?tab=activity#section-2",
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
      await screen.findByRole("heading", { name: "Domain not found" }),
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
