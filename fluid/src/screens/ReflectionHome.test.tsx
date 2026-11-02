/**
 * The Reflection home, and the doors into it: the mode switch on the domain
 * page, the redirect a person with knowledge none gets from every engram address, and the
 * notice that sends a reader to the instance where the Reflection lives.
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

vi.setConfig({ testTimeout: 15000 });

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});
const apiMock = vi.mocked(api);

const missing = () => {
  throw new ApiProblem(404, "not found", "no domain 'eng'");
};

function status(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    enabled: true,
    state: "on",
    home: "local",
    home_url: null,
    right: "curate",
    engrams_visible: true,
    pending_engrams: [],
    ...overrides,
  };
}

const TREE = {
  domain: "eng",
  pact: {
    path: "PACT.md",
    title: "How we write about eng",
    exists: true,
    open_threads: 1,
  },
  nodes: [
    {
      kind: "folder",
      name: "guides",
      path: "guides",
      children: [
        {
          kind: "reflection",
          name: "deploying.md",
          path: "guides/deploying.md",
          title: "Deploying",
          open_threads: 2,
          sections: 3,
        },
      ],
    },
  ],
};

const THREAD = (
  id: string,
  waiting: "people" | "agent",
  authors: string[],
) => ({
  id,
  path: "guides/deploying.md",
  anchor: {
    section: "7f3a9c2e",
    quote: "between 01:30 and 04:00",
    occurrence: 1,
    state: "found",
  },
  status: "open",
  waiting_on: waiting,
  opened_at: "2026-10-01T10:00:00Z",
  age_secs: 3600,
  entries: authors.map((name) => ({
    at: "2026-10-01T10:00:00Z",
    kind: "comment",
    author: { name, agent: name === "bot" },
    text: "Why then?",
  })),
});

const REVIEW = {
  waiting_for_people: [
    {
      path: "guides/deploying.md",
      title: "Deploying",
      threads: [THREAD("t1", "people", ["bot"])],
    },
  ],
  waiting_for_the_agent: [
    {
      path: "guides/deploying.md",
      title: "Deploying",
      threads: [
        THREAD("t2", "agent", ["ben", "ana"]),
        THREAD("t3", "agent", ["ben"]),
      ],
    },
  ],
  queue: {
    total: 2,
    kinds: { feedback: 0, update: 2, review: 0 },
    queue: [],
    guidance: null,
  },
};

/** The summary route's answer for the same threads as {@link REVIEW}. */
const SUMMARY = {
  domain: "eng",
  waiting_for_the_agent: 2,
  waiting_for_people: 1,
  pages_to_update: 2,
  open_by_page: [
    {
      path: "guides/deploying.md",
      title: "Deploying",
      threads: [
        {
          id: "t1",
          quote: "between 01:30 and 04:00",
          waiting_on: "people",
          opened_at: "2026-10-01T10:00:00Z",
          people: [{ name: "bot", agent: true }],
        },
        {
          id: "t2",
          quote: "between 01:30 and 04:00",
          waiting_on: "agent",
          opened_at: "2026-10-01T10:00:00Z",
          people: [
            { name: "ben", agent: false },
            { name: "ana", agent: false },
          ],
        },
        {
          id: "t3",
          quote: "between 01:30 and 04:00",
          waiting_on: "agent",
          opened_at: "2026-10-01T10:00:00Z",
          people: [{ name: "ben", agent: false }],
        },
      ],
    },
  ],
};

/**
 * A person with knowledge `none` and Reflection `contributor` on `eng`: a
 * member of the private `eng`, so it may leave (`member: true`).
 */
function asContributor(routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "ann", role: "viewer" }) }),
      "/domains": () => ({ behavior: [], domains: [] }),
      "/domains/eng/tree": missing,
      "/domains/eng/engrams/runbooks/deploy": missing,
      "/reflection/domains": () => ({
        domains: [
          {
            domain: "eng",
            right: "edit",
            engrams_visible: false,
            home: "here",
            home_url: null,
            open_threads: 3,
          },
        ],
      }),
      "/domains/eng/reflection": () =>
        status({ right: "edit", engrams_visible: false, member: true }),
      "/domains/eng/reflection/tree": () => TREE,
      "/domains/eng/reflection/review": () => REVIEW,
      "/domains/eng/reflection/summary": () => SUMMARY,
      ...routes,
    }),
  );
}

/** An editor on a shared domain, who reads engrams and curates the Reflection. */
function asEditor(routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
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
      "/domains/eng/members": () => ({
        owner: null,
        visibility: "shared",
        members: [],
      }),
      "/vocabulary": () => ({
        domain: "eng",
        tags: [],
        categories: [],
        relation_types: [],
      }),
      "/reflection/domains": () => ({ domains: [] }),
      "/domains/eng/reflection": () => status(),
      "/domains/eng/reflection/tree": () => TREE,
      "/domains/eng/reflection/review": () => REVIEW,
      "/domains/eng/reflection/summary": () => SUMMARY,
      ...routes,
    }),
  );
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the doors into the Reflection", () => {
  it("creates the pact when there is none, for a curator", async () => {
    const put = vi.fn(() => ({
      checksum: "p",
      created: true,
      ids_assigned: [],
    }));
    asEditor({
      "/domains/eng/reflection/tree": () => ({
        ...TREE,
        pact: {
          path: "PACT.md",
          title: "PACT.md",
          exists: false,
          open_threads: 0,
        },
      }),
      "/domains/eng/reflections/PACT.md": (_p, init) =>
        init?.method === "PUT" ? put() : missing(),
    });
    renderApp("/d/eng/reflection");
    await userEvent.click(
      await screen.findByRole("button", { name: "Write the pact" }),
    );
    await waitFor(() => {
      expect(put).toHaveBeenCalledTimes(1);
    });
  });

  it("offers an editor no way to write the pact", async () => {
    asEditor({
      "/domains/eng/reflection": () => status({ right: "edit" }),
      "/domains/eng/reflection/tree": () => ({
        ...TREE,
        pact: {
          path: "PACT.md",
          title: "PACT.md",
          exists: false,
          open_threads: 0,
        },
      }),
    });
    renderApp("/d/eng/reflection");
    expect(await screen.findByText("No pact is written yet.")).toBeVisible();
    expect(screen.queryByRole("button", { name: "Write the pact" })).toBeNull();
  });

  it("a person with knowledge none opening the engram address of a domain lands in its Reflection", async () => {
    asContributor();
    renderApp("/d/eng/e/runbooks/deploy");
    // Exactly what a stranger is told: nothing about whether the engram exists.
    expect(
      await screen.findByRole("heading", { name: "Engram not found" }),
    ).toBeVisible();
    await userEvent.click(
      screen.getByRole("link", { name: "See what eng does hold" }),
    );
    // The Reflection's eyebrow first: the engram view draws its own heading
    // for a moment, until its tree answers 404 and it forwards.
    expect(
      await screen.findByText("Reflection", { selector: "p" }),
    ).toBeVisible();
    expect(
      await screen.findByRole("heading", { name: "eng", level: 1 }),
    ).toBeVisible();
    // A contributor has one mode, so there is nothing to switch between.
    expect(
      screen.queryByRole("navigation", { name: "Domain view" }),
    ).toBeNull();
  });

  it("a stranger still gets domain not found", async () => {
    // A stranger's status read is refused the way the tree is.
    asContributor({
      "/reflection/domains": () => ({ domains: [] }),
      "/domains/eng/reflection": missing,
    });
    renderApp("/d/eng");
    expect(
      await screen.findByRole("heading", { name: "Domain not found" }),
    ).toBeVisible();
  });

  it("offers both modes to a reader of engrams and marks the one on screen", async () => {
    asEditor();
    renderApp("/d/eng/engrams");
    const modes = await screen.findByRole("navigation", {
      name: "Domain view",
    });
    expect(
      within(modes).getByRole("link", { name: "Engrams" }),
    ).toHaveAttribute("aria-current", "page");
    await userEvent.click(
      within(modes).getByRole("link", { name: "Reflection" }),
    );
    const again = await screen.findByRole("navigation", {
      name: "Domain view",
    });
    expect(
      within(again).getByRole("link", { name: "Reflection" }),
    ).toHaveAttribute("aria-current", "page");
  });

  it("draws no switch while the Reflection is off", async () => {
    asEditor({
      "/domains/eng/reflection": () => status({ state: "off", enabled: false }),
    });
    renderApp("/d/eng");
    expect(
      await screen.findByRole("heading", { name: "eng", level: 1 }),
    ).toBeVisible();
    await waitFor(() => {
      expect(apiMock).toHaveBeenCalledWith("/domains/eng/reflection");
    });
    expect(
      screen.queryByRole("navigation", { name: "Domain view" }),
    ).toBeNull();
  });

  it("turns the Reflection half of the switch into a link to the home instance when the Reflection lives elsewhere", async () => {
    asEditor({
      "/domains/eng/reflection": () =>
        status({
          state: "elsewhere",
          home: "elsewhere",
          home_url: "https://team.example.com",
        }),
    });
    renderApp("/d/eng");
    const modes = await screen.findByRole("navigation", {
      name: "Domain view",
    });
    expect(
      within(modes).getByRole("link", { name: "Engrams" }),
    ).toHaveAttribute("aria-current", "page");
    const away = within(modes).getByRole("link", {
      name: "Reflection (opens on another instance)",
    });
    expect(away).toHaveAttribute("href", "https://team.example.com");
    expect(away).not.toHaveAttribute("aria-current");
    // The bar says it now; the domain page no longer carries the sentence.
    expect(screen.queryByText(/This domain's Reflection lives at/)).toBeNull();
  });

  it("never turns a home address that is not a web address into a link", async () => {
    asEditor({
      "/domains/eng/reflection": () =>
        status({
          state: "elsewhere",
          home: "elsewhere",
          home_url: "javascript:alert(1)",
        }),
    });
    renderApp("/d/eng");
    expect(
      await screen.findByRole("heading", { name: "eng", level: 1 }),
    ).toBeVisible();
    await waitFor(() => {
      expect(apiMock).toHaveBeenCalledWith("/domains/eng/reflection");
    });
    // Nowhere safe to go, so no switch at all rather than a dead half, and
    // the domain page says where it lives in plain words instead.
    expect(
      screen.queryByRole("navigation", { name: "Domain view" }),
    ).toBeNull();
    expect(
      await screen.findByText(
        "This domain's Reflection lives at javascript:alert(1)",
      ),
    ).toBeVisible();
    expect(document.querySelector('a[href^="javascript"]')).toBeNull();
  });

  it("leaves a person with knowledge none on domain not found when the Reflection lives elsewhere", async () => {
    asContributor({
      "/reflection/domains": () => ({
        domains: [
          {
            domain: "eng",
            state: "elsewhere",
            right: "edit",
            engrams_visible: false,
            home: "elsewhere",
            home_url: "https://team.example.com",
            open_threads: 0,
          },
        ],
      }),
      "/domains/eng/reflection": () =>
        status({
          state: "elsewhere",
          right: "edit",
          engrams_visible: false,
          home: "elsewhere",
          home_url: "https://team.example.com",
        }),
    });
    renderApp("/d/eng");
    expect(
      await screen.findByRole("heading", { name: "Domain not found" }),
    ).toBeVisible();
  });
  it("never sends a reader of engrams to the Reflection from a missing address", async () => {
    asEditor({
      "/domains/eng/tree": missing,
      "/reflection/domains": () => ({
        domains: [
          {
            domain: "eng",
            right: "curate",
            engrams_visible: true,
            home: "here",
            home_url: null,
            open_threads: 0,
          },
        ],
      }),
    });
    renderApp("/d/eng?path=gone");
    expect(
      await screen.findByRole("heading", { name: "Domain not found" }),
    ).toBeVisible();
  });
});

describe("the Reflection home", () => {
  it("names the pending engrams while the switch waits for them", async () => {
    asEditor({
      "/domains/eng/reflection": () =>
        status({
          state: "pending",
          pending_engrams: ["reflections/old-note.md"],
        }),
    });
    renderApp("/d/eng/reflection");
    expect(
      await screen.findByText(
        "The Reflection waits until these engrams leave reflections/:",
      ),
    ).toBeVisible();
    expect(screen.getByText("reflections/old-note.md")).toBeVisible();
  });

  it("links the pact and the review screen with the open thread count", async () => {
    asEditor();
    renderApp("/d/eng/reflection");
    // The sidebar names the pact too, so the home's own links are read
    // inside the main region.
    const main = await screen.findByRole("main");
    expect(
      await within(main).findByRole("link", { name: "How we write about eng" }),
    ).toHaveAttribute("href", "/d/eng/reflection/PACT.md");
    const review = within(main).getByRole("link", { name: "Review" });
    expect(review).toHaveAttribute("href", "/d/eng/reflection/review");
    expect(review.closest("section")).toHaveTextContent("3 open threads");
  });

  it("says in the Review card what waits", async () => {
    asEditor();
    renderApp("/d/eng/reflection");
    const card = await within(await screen.findByRole("main")).findByRole(
      "region",
      { name: "Review" },
    );
    expect(
      await within(card).findByText(
        "2 threads wait for the agent. 1 thread waits for people. 2 pages need an update.",
      ),
    ).toBeVisible();
  });

  it("says nothing waits when the review is empty", async () => {
    asEditor({
      "/domains/eng/reflection/summary": () => ({
        domain: "eng",
        waiting_for_the_agent: 0,
        waiting_for_people: 0,
        pages_to_update: 0,
        open_by_page: [],
      }),
    });
    renderApp("/d/eng/reflection");
    expect(await screen.findByText("Nothing waits for review.")).toBeVisible();
    expect(
      screen.queryByRole("list", { name: "Pages with open threads" }),
    ).toBeNull();
  });

  it("groups the open threads by page with the people involved", async () => {
    asEditor();
    renderApp("/d/eng/reflection");
    const list = await screen.findByRole("list", {
      name: "Pages with open threads",
    });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(1);
    expect(
      within(rows[0] as HTMLElement).getByRole("link", { name: "Deploying" }),
    ).toHaveAttribute("href", "/d/eng/reflection/guides/deploying.md");
    expect(rows[0]).toHaveTextContent("bot (agent), ben, ana");
    expect(rows[0]).toHaveTextContent("3 open");
    expect(screen.queryByText(/on 1 page/)).toBeNull();
  });

  it("reads the summary route and never the full review", async () => {
    asEditor();
    renderApp("/d/eng/reflection");
    await screen.findByRole("list", { name: "Pages with open threads" });
    const paths = apiMock.mock.calls.map(([path]) => String(path));
    expect(paths).toContain("/domains/eng/reflection/summary");
    expect(paths.some((path) => path.includes("/reflection/review"))).toBe(
      false,
    );
  });

  it("falls back to the tree's count when an older server has no summary route", async () => {
    asEditor({
      "/domains/eng/reflection/summary": missing,
    });
    renderApp("/d/eng/reflection");
    const card = await within(await screen.findByRole("main")).findByRole(
      "region",
      { name: "Review" },
    );
    await within(
      await screen.findByRole("list", { name: "All pages" }),
    ).findByText("Deploying");
    await waitFor(() => {
      expect(
        apiMock.mock.calls.some(
          ([path]) => String(path) === "/domains/eng/reflection/summary",
        ),
      ).toBe(true);
    });
    expect(card).toHaveTextContent("3 open threads");
    expect(
      screen.queryByRole("list", { name: "Pages with open threads" }),
    ).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("falls back to the tree's count when the review is refused, with no groups and no alert", async () => {
    asEditor({
      "/domains/eng/reflection/summary": () => {
        throw new ApiProblem(500, "failed", "the review broke");
      },
    });
    renderApp("/d/eng/reflection");
    const card = await within(await screen.findByRole("main")).findByRole(
      "region",
      { name: "Review" },
    );
    await within(
      await screen.findByRole("list", { name: "All pages" }),
    ).findByText("Deploying");
    expect(card).toHaveTextContent("3 open threads");
    expect(
      screen.queryByRole("list", { name: "Pages with open threads" }),
    ).toBeNull();
    expect(screen.queryByRole("heading", { name: "Open threads" })).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByText(/the review broke/)).toBeNull();
  });

  it("lists every page with its folder, sections and open threads", async () => {
    asEditor();
    renderApp("/d/eng/reflection");
    const list = await screen.findByRole("list", { name: "All pages" });
    const row = within(list).getByRole("listitem");
    expect(row).toHaveTextContent("Deploying");
    expect(row).toHaveTextContent("guides");
    expect(row).toHaveTextContent("3 sections");
    expect(row).toHaveTextContent("2 open threads");
  });

  it("offers New page nowhere on the home", async () => {
    asEditor();
    renderApp("/d/eng/reflection");
    const main = await screen.findByRole("main");
    await within(main).findByRole("list", { name: "All pages" });
    expect(within(main).queryByText(/New page/)).toBeNull();
  });

  it("writes the pact from its card for a curator", async () => {
    asEditor({
      "/domains/eng/reflection/tree": () => ({
        ...TREE,
        pact: {
          path: "PACT.md",
          title: "PACT.md",
          exists: false,
          open_threads: 0,
        },
      }),
    });
    renderApp("/d/eng/reflection");
    const card = await within(await screen.findByRole("main")).findByRole(
      "region",
      { name: "Pact" },
    );
    expect(
      await within(card).findByRole("button", { name: "Write the pact" }),
    ).toBeVisible();
    expect(within(card).getByText("How we write about eng")).toBeVisible();
  });

  it("shows the pact card to an editor without the write button", async () => {
    asEditor({
      "/domains/eng/reflection": () => status({ right: "edit" }),
      "/domains/eng/reflection/tree": () => ({
        ...TREE,
        pact: {
          path: "PACT.md",
          title: "PACT.md",
          exists: false,
          open_threads: 0,
        },
      }),
    });
    renderApp("/d/eng/reflection");
    const card = await within(await screen.findByRole("main")).findByRole(
      "region",
      { name: "Pact" },
    );
    expect(
      await within(card).findByText("No pact is written yet."),
    ).toBeVisible();
    expect(within(card).queryByRole("button")).toBeNull();
  });

  it("searches the Reflection in the mode asked for, says which ran, and never shows an id line", async () => {
    const asked: string[] = [];
    asEditor({
      "/domains/eng/reflection/search": (path) => {
        asked.push(path);
        return {
          domain: "eng",
          query: "night",
          mode: "text",
          total: 1,
          page: 1,
          limit: 20,
          count: 1,
          hits: [
            {
              path: "guides/deploying.md",
              title: "Deploying",
              section: "7f3a9c2e",
              // A comment in the heading is a note for editors, never shown.
              heading: "Night deploys <!-- checked in March -->",
              // The shipped snippet (core Task 30) carries no heading line;
              // an id line is stripped anyway, as defence.
              snippet: "Deploys run at night.\n<!-- r:0b1c2d3e -->",
            },
          ],
        };
      },
    });
    renderApp("/d/eng/reflection");
    // Addressed the way the engram search test addresses its own Mode select.
    await userEvent.selectOptions(
      await screen.findByLabelText("Mode"),
      "semantic",
    );
    await userEvent.type(
      screen.getByRole("searchbox", { name: "Search the Reflection" }),
      "night",
    );
    await userEvent.click(screen.getByRole("button", { name: "Search" }));
    const hit = await screen.findByRole("link", {
      name: "Deploying: Night deploys",
    });
    expect(hit).toHaveAttribute(
      "href",
      "/d/eng/reflection/guides/deploying.md#s-7f3a9c2e",
    );
    expect(asked.at(-1)).toBe(
      "/domains/eng/reflection/search?q=night&limit=20&search_type=semantic",
    );
    expect(
      screen.getByText(
        "Asked for semantic, ran as text: the pages are not embedded yet.",
      ),
    ).toBeVisible();
    expect(screen.queryByText(/r:0b1c2d3e/)).toBeNull();
    expect(screen.queryByText(/checked in March/)).toBeNull();
  });

  it("links a hit on the intro to the page's top and pages through a long answer", async () => {
    asEditor({
      "/domains/eng/reflection/search": () => ({
        domain: "eng",
        query: "night",
        mode: "hybrid",
        total: 25,
        page: 1,
        limit: 20,
        count: 1,
        hits: [
          {
            path: "a.md",
            title: "A",
            section: "_top",
            heading: "",
            snippet: "Intro.",
          },
        ],
      }),
    });
    renderApp("/d/eng/reflection?q=night");
    expect(await screen.findByRole("link", { name: "A" })).toHaveAttribute(
      "href",
      "/d/eng/reflection/a.md#s-_top",
    );
    expect(screen.getByText("25 hits, ranked by hybrid.")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Next page" }));
    await waitFor(() => {
      expect(apiMock).toHaveBeenCalledWith(
        "/domains/eng/reflection/search?q=night&limit=20&page=2",
      );
    });
  });

  it("says once where a Reflection that lives elsewhere is", async () => {
    asEditor({
      "/domains/eng/reflection": () =>
        status({
          state: "elsewhere",
          home: "elsewhere",
          home_url: "https://team.example.com",
        }),
    });
    renderApp("/d/eng/reflection");
    expect(
      await screen.findByRole("link", { name: "https://team.example.com" }),
    ).toBeVisible();
    expect(
      screen.getAllByText(/This domain's Reflection lives at/),
    ).toHaveLength(1);
    // The bar keeps the switch, its Reflection half pointing at the home.
    expect(
      within(screen.getByRole("navigation", { name: "Domain view" })).getByRole(
        "link",
        { name: "Reflection (opens on another instance)" },
      ),
    ).toHaveAttribute("href", "https://team.example.com");
    // Still the mode on screen, though it links out.
    expect(
      screen.getByRole("link", {
        name: "Reflection (opens on another instance)",
      }),
    ).toHaveAttribute("aria-current", "true");
  });

  it("says a homeless Reflection has no home yet, never that it is off", async () => {
    asEditor({
      "/domains/eng/reflection": () =>
        status({ state: "homeless", home: null }),
    });
    renderApp("/d/eng/reflection");
    expect(await screen.findByText(/Reflection has no home yet/)).toBeVisible();
    expect(screen.queryByText(/is off for this domain/)).toBeNull();
  });

  it("names the instance that serves the Reflection, never that it is off", async () => {
    asEditor({
      "/domains/eng/reflection": () =>
        status({
          state: "hosted_elsewhere",
          home: null,
          host_label: "office-mac",
        }),
    });
    renderApp("/d/eng/reflection");
    expect(
      await screen.findByText(
        "This domain's Reflection is served by another instance: office-mac.",
      ),
    ).toBeVisible();
    expect(screen.queryByText(/is off for this domain/)).toBeNull();
  });

  it("says a rename holds the Reflection when no instance is named", async () => {
    asEditor({
      "/domains/eng/reflection": () =>
        status({ state: "hosted_elsewhere", home: null, host_label: null }),
    });
    renderApp("/d/eng/reflection");
    expect(
      await screen.findByText(
        "This domain is being renamed. Its Reflection opens again when the rename is done.",
      ),
    ).toBeVisible();
    expect(screen.queryByText(/served by another instance/)).toBeNull();
  });

  it("lets a contributor leave the domain", async () => {
    const left = vi.fn(() => undefined);
    asContributor({
      "/domains/eng/members/ann": (_path, init) =>
        init?.method === "DELETE" ? left() : missing(),
    });
    renderApp("/d/eng/reflection");
    await userEvent.click(
      await screen.findByRole("button", { name: "Leave this domain" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Confirm leave" }),
    );
    await waitFor(() => {
      expect(left).toHaveBeenCalledTimes(1);
    });
    expect(
      await screen.findByRole("heading", { name: "Home", level: 1 }),
    ).toBeVisible();
  });

  it("offers no leave where there is no membership: knowledge none set for every shared domain", async () => {
    asContributor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "nora", role: "none" }) }),
      "/domains/eng/reflection": () =>
        status({ right: "edit", engrams_visible: false, member: false }),
    });
    renderApp("/d/eng");
    expect(
      await within(await screen.findByRole("main")).findByRole("link", {
        name: "How we write about eng",
      }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Leave this domain" }),
    ).toBeNull();
  });

  it("offers no leave to a reader of engrams, whose membership lives on the domain page", async () => {
    asEditor();
    renderApp("/d/eng/reflection");
    expect(
      await within(await screen.findByRole("main")).findByRole("link", {
        name: "How we write about eng",
      }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Leave this domain" }),
    ).toBeNull();
  });
});

describe("the Reflection home, fix round 1", () => {
  /** One page of a long answer: `total` hits, 20 a page, one drawn. */
  function longAnswer(
    total: number,
    hits: unknown[] = [
      {
        path: "a.md",
        title: "A",
        section: "_top",
        heading: "",
        snippet: "Intro.",
      },
    ],
  ) {
    return () => ({
      domain: "eng",
      query: "night",
      mode: "hybrid",
      total,
      page: 1,
      limit: 20,
      count: hits.length,
      hits,
    });
  }

  it("says what a pending Reflection waits for when it may not name the engrams", async () => {
    asContributor({
      "/domains/eng/reflection": () =>
        status({
          state: "pending",
          right: "edit",
          engrams_visible: false,
          member: true,
          pending_engrams: [],
        }),
    });
    renderApp("/d/eng/reflection");
    expect(
      await screen.findByText(
        "The Reflection waits until some engrams are moved out of reflections/. A curator does that.",
      ),
    ).toBeVisible();
    expect(screen.queryByText(/leave reflections\/:/)).toBeNull();
    // The sidebar tree is a list of its own; the screen draws none.
    expect(within(screen.getByRole("main")).queryByRole("list")).toBeNull();
  });

  it("lets a contributor leave while the Reflection is not open", async () => {
    asContributor({
      "/domains/eng/reflection": () =>
        status({
          state: "homeless",
          home: null,
          right: "edit",
          engrams_visible: false,
          member: true,
        }),
    });
    renderApp("/d/eng/reflection");
    expect(await screen.findByText(/Reflection has no home yet/)).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Leave this domain" }),
    ).toBeVisible();
  });

  it("says domain not found only when the status is not found", async () => {
    asEditor({ "/domains/eng/reflection": missing });
    renderApp("/d/eng/reflection");
    expect(
      await screen.findByRole("heading", { name: "Domain not found" }),
    ).toBeVisible();
  });

  it("shows any other failure as the server's own words", async () => {
    asEditor({
      "/domains/eng/reflection": () => {
        throw new ApiProblem(500, "internal", "the index is not open");
      },
    });
    renderApp("/d/eng/reflection");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "the index is not open",
    );
    expect(
      screen.queryByRole("heading", { name: "Domain not found" }),
    ).toBeNull();
  });

  it("says the Reflection is off and who switches it on", async () => {
    asEditor({
      "/domains/eng/reflection": () => status({ state: "off", enabled: false }),
    });
    renderApp("/d/eng/reflection");
    expect(
      await screen.findByText(
        "The Reflection is off for this domain. Its owner switches it on in the domain policies.",
      ),
    ).toBeVisible();
    expect(
      screen.queryByRole("searchbox", { name: "Search the Reflection" }),
    ).toBeNull();
  });

  it("says no page matches when the answer is empty", async () => {
    asEditor({ "/domains/eng/reflection/search": longAnswer(0, []) });
    renderApp("/d/eng/reflection?q=night");
    expect(await screen.findByText("No page matches.")).toBeVisible();
    expect(screen.queryByText(/hits?, ranked by/)).toBeNull();
  });

  it("offers no previous page on the first page and no next page on the last", async () => {
    asEditor({ "/domains/eng/reflection/search": longAnswer(25) });
    renderApp("/d/eng/reflection?q=night");
    expect(
      await screen.findByRole("button", { name: "Next page" }),
    ).toBeVisible();
    expect(screen.queryByRole("button", { name: "Previous page" })).toBeNull();
  });

  it("goes back a page and offers no next page on the last one", async () => {
    asEditor({ "/domains/eng/reflection/search": longAnswer(25) });
    renderApp("/d/eng/reflection?q=night&page=2");
    await userEvent.click(
      await screen.findByRole("button", { name: "Previous page" }),
    );
    await waitFor(() => {
      expect(apiMock).toHaveBeenCalledWith(
        "/domains/eng/reflection/search?q=night&limit=20",
      );
    });
  });

  it("draws the last page without a next page", async () => {
    asEditor({ "/domains/eng/reflection/search": longAnswer(25) });
    renderApp("/d/eng/reflection?q=night&page=2");
    expect(
      await screen.findByRole("button", { name: "Previous page" }),
    ).toBeVisible();
    expect(screen.queryByRole("button", { name: "Next page" })).toBeNull();
  });

  it("says one thing on a page past the last one, and offers the way back", async () => {
    asEditor({ "/domains/eng/reflection/search": longAnswer(25, []) });
    renderApp("/d/eng/reflection?q=night&page=9");
    expect(await screen.findByText("No page matches.")).toBeVisible();
    expect(screen.queryByText("25 hits, ranked by hybrid.")).toBeNull();
    await userEvent.click(
      screen.getByRole("button", { name: "Previous page" }),
    );
    await waitFor(() => {
      expect(apiMock).toHaveBeenCalledWith(
        "/domains/eng/reflection/search?q=night&limit=20&page=2",
      );
    });
  });

  it("draws two hits on one path without a section id apart", async () => {
    const hit = {
      path: "a.md",
      title: "A",
      section: null,
      heading: "",
      snippet: "One.",
    };
    asEditor({
      "/domains/eng/reflection/search": longAnswer(2, [
        hit,
        { ...hit, snippet: "Two." },
      ]),
    });
    const errors = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    renderApp("/d/eng/reflection?q=night");
    expect(await screen.findAllByRole("link", { name: "A" })).toHaveLength(2);
    expect(
      errors.mock.calls.some((call) => String(call[0]).includes("same key")),
    ).toBe(false);
    errors.mockRestore();
  });

  it("keeps the search box in step with the address when it changes under it", async () => {
    asEditor({ "/domains/eng/reflection/search": longAnswer(1) });
    renderApp("/d/eng/reflection?q=night&search_type=text");
    const field = await screen.findByRole("searchbox", {
      name: "Search the Reflection",
    });
    expect(field).toHaveValue("night");
    expect(screen.getByLabelText("Mode")).toHaveValue("text");
    // The switch's own link is a new address on the same screen, as a step
    // back or forward in the history is.
    await userEvent.click(
      within(screen.getByRole("navigation", { name: "Domain view" })).getByRole(
        "link",
        { name: "Reflection" },
      ),
    );
    await waitFor(() => {
      expect(
        screen.getByRole("searchbox", { name: "Search the Reflection" }),
      ).toHaveValue("");
    });
    expect(screen.getByLabelText("Mode")).toHaveValue("hybrid");
  });
});

/**
 * The mode switch lives in the top bar on every screen inside a domain (spec
 * 4.2): the Engrams half is current on the domain page, an engram and the
 * editor, the Reflection half on every Reflection screen. Only the half
 * whose address is on screen says "page"; the other screens of a mode say
 * "true", because the reader is in that mode but not on that page.
 */
describe("the mode switch in the top bar", () => {
  async function modes(): Promise<HTMLElement> {
    // The bar is the header around the switcher; a screen's own header is
    // a banner to the queries too, so it is not asked for by role.
    const bar = (
      await screen.findByRole("button", { name: /switch domain$/ })
    ).closest("header");
    return within(bar ?? document.body).findByRole("navigation", {
      name: "Domain view",
    });
  }

  it("draws no switch outside a domain", async () => {
    asEditor({
      "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
    });
    renderApp("/");
    expect(
      await screen.findByRole("button", { name: "All domains, switch domain" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("navigation", { name: "Domain view" }),
    ).toBeNull();
  });

  it("marks Engrams as the mode of an engram page", async () => {
    asEditor({
      "/domains/eng/engrams/alpha": () => ({
        domain: "eng",
        permalink: "alpha",
        title: "Alpha",
        url: "crystalline://eng/alpha",
        content: "# Alpha\n",
        frontmatter: {},
        observations: [],
        relations: [],
        links: [],
      }),
    });
    renderApp("/d/eng/e/alpha");
    const nav = await modes();
    expect(within(nav).getByRole("link", { name: "Engrams" })).toHaveAttribute(
      "aria-current",
      "true",
    );
    expect(
      within(nav).getByRole("link", { name: "Reflection" }),
    ).not.toHaveAttribute("aria-current");
  });

  it("marks Reflection on the Reflection home as the page itself", async () => {
    asEditor();
    renderApp("/d/eng/reflection");
    const nav = await modes();
    expect(
      within(nav).getByRole("link", { name: "Reflection" }),
    ).toHaveAttribute("aria-current", "page");
    // Moved, not copied: the screen's own header no longer carries one.
    expect(
      within(screen.getByRole("main")).queryByRole("navigation", {
        name: "Domain view",
      }),
    ).toBeNull();
  });

  it("keeps the switch on the review screen, with Reflection as its mode", async () => {
    asEditor({
      "/domains/eng/reflection/review": () => ({
        domain: "eng",
        threads: [],
        queue: [],
      }),
    });
    renderApp("/d/eng/reflection/review");
    const nav = await modes();
    expect(
      within(nav).getByRole("link", { name: "Reflection" }),
    ).toHaveAttribute("aria-current", "true");
    expect(within(nav).getByRole("link", { name: "Engrams" })).toHaveAttribute(
      "href",
      "/d/eng/engrams",
    );
  });

  it.each([
    ["pending", { state: "pending", home: null }],
    ["homeless", { state: "homeless", home: null }],
    ["hosted_elsewhere", { state: "hosted_elsewhere", host_label: "b" }],
  ])("keeps both halves while the Reflection is %s", async (_, over) => {
    asEditor({ "/domains/eng/reflection": () => status(over) });
    renderApp("/d/eng");
    const nav = await modes();
    expect(
      within(nav).getByRole("link", { name: "Reflection" }),
    ).toHaveAttribute("href", "/d/eng/reflection");
  });

  it.each([
    ["the engram editor", "/d/eng/edit/alpha", "Engrams"],
    [
      "a Reflection page",
      "/d/eng/reflection/guides/deploying.md",
      "Reflection",
    ],
    [
      "the Reflection editor",
      "/d/eng/reflection-edit/guides/deploying.md",
      "Reflection",
    ],
  ])("marks the mode of %s as current", async (_, path, mode) => {
    asEditor();
    renderApp(path);
    const nav = await modes();
    expect(within(nav).getByRole("link", { name: mode })).toHaveAttribute(
      "aria-current",
      "true",
    );
  });

  it("draws no switch for a person with knowledge none, who has one mode", async () => {
    asContributor();
    renderApp("/d/eng/reflection");
    expect(
      await screen.findByRole("heading", { name: "eng", level: 1 }),
    ).toBeVisible();
    await waitFor(() => {
      expect(apiMock).toHaveBeenCalledWith("/domains/eng/reflection");
    });
    expect(
      screen.queryByRole("navigation", { name: "Domain view" }),
    ).toBeNull();
  });
});

describe("the last reflect run", () => {
  const LOOP = {
    last_run: null,
    running: null,
    left_for_person: 0,
    judgment_waiting: 0,
    delivery: null,
  };
  const ago = (hours: number) =>
    new Date(Date.now() - hours * 3600 * 1000).toISOString();

  it("says when it last ran and links what waits for a person", async () => {
    asEditor({
      "/domains/eng/automation": () => ({
        reflect: {
          ...LOOP,
          left_for_person: 3,
          last_run: {
            started_at: ago(2),
            ended_at: ago(2),
            runner: "n8n",
            account: null,
            outcome: "done",
            done: 4,
            left: 3,
            judgment_waiting: 0,
            summary: "",
            pr_url: null,
          },
        },
        evolve: LOOP,
      }),
    });
    renderApp("/d/eng/reflection");
    const card = await within(await screen.findByRole("main")).findByRole(
      "region",
      { name: "Review" },
    );
    expect(
      await within(card).findByText("Last reflected 2 hours ago by n8n: done."),
    ).toBeVisible();
    expect(
      within(card).getByRole("link", { name: "3 waiting for a person" }),
    ).toHaveAttribute("href", "/d/eng/reflection/review");
    // Only the reflect loop is asked for.
    expect(apiMock.mock.calls.map((call) => call[0])).toContain(
      "/domains/eng/automation?loop=reflect",
    );
  });

  it("links nothing when the caller got no count", async () => {
    asEditor({
      "/domains/eng/automation": () => ({
        reflect: {
          ...LOOP,
          left_for_person: null,
          last_run: {
            started_at: ago(2),
            ended_at: ago(2),
            runner: "n8n",
            outcome: "done",
            done: 4,
            left: 3,
          },
        },
        evolve: null,
      }),
    });
    renderApp("/d/eng/reflection");
    const card = await within(await screen.findByRole("main")).findByRole(
      "region",
      { name: "Review" },
    );
    expect(
      await within(card).findByText("Last reflected 2 hours ago by n8n: done."),
    ).toBeVisible();
    expect(within(card).queryByText(/waiting for a person/)).toBeNull();
  });

  it("says nothing when the loop never ran", async () => {
    asEditor({
      "/domains/eng/automation": () => ({ reflect: LOOP, evolve: LOOP }),
    });
    renderApp("/d/eng/reflection");
    const card = await within(await screen.findByRole("main")).findByRole(
      "region",
      { name: "Review" },
    );
    await within(card).findByText(/waits? for|Nothing waits/);
    expect(within(card).queryByText(/Last reflected/)).toBeNull();
    expect(within(card).queryByText(/waiting for a person/)).toBeNull();
  });
});

const UNLINKED_HINT =
  "Connect GitHub on your profile to contribute, and to open the private domains you can read on GitHub.";

/** The link read of an account with no GitHub link, on an instance with an App. */
const unlinked = () => ({
  linked: false,
  app_configured: true,
  token_live: false,
});

describe("an account with no GitHub link", () => {
  it("is told how to open a private GitHub domain from its Reflection", async () => {
    asContributor({
      "/domains/eng/reflection": () =>
        status({
          right: "read",
          engrams_visible: false,
          governance: "github",
          private: true,
        }),
      "/me/github-link": unlinked,
    });
    renderApp("/d/eng/reflection");
    expect(await screen.findByText(UNLINKED_HINT)).toBeVisible();
  });

  it("is told nothing where Crystalline decides", async () => {
    asContributor({
      "/domains/eng/reflection": () =>
        status({ right: "read", engrams_visible: false, private: true }),
      "/me/github-link": unlinked,
    });
    renderApp("/d/eng/reflection");
    await screen.findByRole("heading", { name: "eng", level: 1 });
    expect(screen.queryByText(UNLINKED_HINT)).toBeNull();
  });
});
