/**
 * A domain's settings screen (`/d/:domain/settings`): every card the front
 * page drew before the MANIFEST became the page, under the same gates, and
 * the configuration sections with the way to start one. Each card has its
 * own suite; this file pins what the screen itself owns: that nothing moved
 * off the front page is lost, the order, the members gate, the start of a
 * configuration section, and the palette row the front page hands over.
 */

import { act, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter, useLocation } from "react-router";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import App from "../App";
import { ApiProblem, api } from "../api/client";
import type { Answer } from "../test/harness";
import {
  answersFor,
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

const apiMock = vi.mocked(api);

const MANIFEST = "# eng\n\nWhat this domain is for.\n";

function serve(
  routes: Record<string, Answer> = {},
  role: "admin" | "editor" = "admin",
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role }) }),
      "/domains": domainsResponse,
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: MANIFEST,
        sections: manifestSectionsResponse(),
      }),
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
      "/vocabulary": () => ({ domain: "eng", tags: [] }),
      "/domains/eng/members": () => ({
        owner: null,
        visibility: "shared",
        members: [],
      }),
      ...routes,
    }),
  );
}

/** Whether one element is drawn before another, in document order. */
function precedes(before: Element, after: Element): boolean {
  return (
    (before.compareDocumentPosition(after) &
      Node.DOCUMENT_POSITION_FOLLOWING) !==
    0
  );
}

beforeEach(() => {
  apiMock.mockReset();
  localStorage.clear();
});

describe("the domain settings screen", () => {
  it("holds every card that left the front page, in order", async () => {
    serve();

    renderApp("/d/eng/settings");

    expect(
      await screen.findByRole("heading", { level: 1, name: "eng" }),
    ).toBeVisible();
    expect(
      screen.getByRole("link", { name: "Back to the domain" }),
    ).toHaveAttribute("href", "/d/eng/engrams");
    const members = await screen.findByRole("region", { name: "Members" });
    const review = await screen.findByRole("region", { name: "Review mode" });
    const policies = await screen.findByRole("region", {
      name: "Domain policies",
    });
    const provisioning = screen.getByRole("region", { name: "Provisioning" });
    const aliases = screen.getByRole("region", { name: "Tag aliases" });
    const backup = screen.getByRole("region", { name: "Backup" });
    const danger = await screen.findByRole("region", { name: "Danger zone" });
    const order = [
      members,
      review,
      policies,
      provisioning,
      aliases,
      backup,
      danger,
    ];
    for (let i = 1; i < order.length; i += 1) {
      expect(precedes(order[i - 1] as Element, order[i] as Element)).toBe(true);
    }
    expect(
      within(backup).getByRole("link", { name: "Download archive" }),
    ).toBeVisible();
  });

  it("lists the sections in an index whose links match the hashes", async () => {
    serve();

    renderApp("/d/eng/settings");

    const nav = await screen.findByRole("navigation", {
      name: "Settings sections",
    });
    const links = within(nav).getAllByRole("link");
    expect(links.map((link) => link.textContent)).toEqual([
      "Members",
      "Review mode",
      "Domain policies",
      "Provisioning",
      "Tag aliases",
      "Backup",
      "Danger zone",
    ]);
    expect(links.map((link) => link.getAttribute("href"))).toEqual([
      "/d/eng/settings#members",
      "/d/eng/settings#review-mode",
      "/d/eng/settings#policies",
      "/d/eng/settings#provisioning",
      "/d/eng/settings#tag-aliases",
      "/d/eng/settings#backup",
      "/d/eng/settings#danger-zone",
    ]);
    for (const link of links) {
      const id = (link.getAttribute("href") ?? "").split("#")[1] ?? "";
      expect(document.getElementById(id)).not.toBeNull();
    }
    // Nothing is current until the reader scrolls or presses one, except
    // the first section.
    expect(links[0]).toHaveAttribute("aria-current", "location");
  });

  it("marks a pressed section as current", async () => {
    serve();

    renderApp("/d/eng/settings");

    const nav = await screen.findByRole("navigation", {
      name: "Settings sections",
    });
    await userEvent.click(within(nav).getByRole("link", { name: "Backup" }));
    expect(within(nav).getByRole("link", { name: "Backup" })).toHaveAttribute(
      "aria-current",
      "location",
    );
    expect(
      within(nav).getByRole("link", { name: "Members" }),
    ).not.toHaveAttribute("aria-current");
  });

  it("follows the scroll: the first section in view is the current one", async () => {
    let callback: IntersectionObserverCallback = () => {};
    class FakeObserver {
      constructor(cb: IntersectionObserverCallback) {
        callback = cb;
      }
      observe() {}
      disconnect() {}
      unobserve() {}
      takeRecords() {
        return [];
      }
    }
    vi.stubGlobal("IntersectionObserver", FakeObserver);
    try {
      serve();
      renderApp("/d/eng/settings");
      const nav = await screen.findByRole("navigation", {
        name: "Settings sections",
      });
      const target = document.getElementById("policies") as HTMLElement;
      act(() => {
        callback(
          [
            {
              isIntersecting: true,
              target,
            } as unknown as IntersectionObserverEntry,
          ],
          {} as IntersectionObserver,
        );
      });
      expect(
        within(nav).getByRole("link", { name: "Domain policies" }),
      ).toHaveAttribute("aria-current", "location");
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("scrolls to the section on every press, the one in the hash too, and focuses its heading", async () => {
    const spy = vi.fn();
    Element.prototype.scrollIntoView = spy;
    try {
      serve();
      renderApp("/d/eng/settings#backup");
      const nav = await screen.findByRole("navigation", {
        name: "Settings sections",
      });
      const backup = within(nav).getByRole("link", { name: "Backup" });
      // The hash already names it: the hash effect has nothing to react to.
      expect(backup).toHaveAttribute("aria-current", "location");
      spy.mockClear();
      await userEvent.click(backup);
      await userEvent.click(backup);
      expect(spy).toHaveBeenCalledTimes(2);
      expect(spy.mock.contexts[1]).toBe(document.getElementById("backup"));
      expect(
        within(document.getElementById("backup") as HTMLElement).getByRole(
          "heading",
          { name: "Backup" },
        ),
      ).toHaveFocus();
    } finally {
      Reflect.deleteProperty(Element.prototype, "scrollIntoView");
    }
  });

  it("keeps a pressed section current until the reader scrolls on their own", async () => {
    let callback: IntersectionObserverCallback = () => {};
    class FakeObserver {
      constructor(cb: IntersectionObserverCallback) {
        callback = cb;
      }
      observe() {}
      disconnect() {}
      unobserve() {}
      takeRecords() {
        return [];
      }
    }
    vi.stubGlobal("IntersectionObserver", FakeObserver);
    try {
      serve();
      renderApp("/d/eng/settings");
      const nav = await screen.findByRole("navigation", {
        name: "Settings sections",
      });
      await userEvent.click(within(nav).getByRole("link", { name: "Backup" }));
      // The scroll the press causes reports another section in view.
      act(() => {
        callback(
          [
            {
              isIntersecting: true,
              target: document.getElementById("members"),
            } as unknown as IntersectionObserverEntry,
          ],
          {} as IntersectionObserver,
        );
      });
      expect(within(nav).getByRole("link", { name: "Backup" })).toHaveAttribute(
        "aria-current",
        "location",
      );
      // The reader's own wheel hands the highlight back to the scroll.
      act(() => {
        window.dispatchEvent(new Event("wheel"));
      });
      expect(
        within(nav).getByRole("link", { name: "Members" }),
      ).toHaveAttribute("aria-current", "location");
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("makes the last section current when the page is scrolled to the bottom", async () => {
    serve();
    renderApp("/d/eng/settings");
    const nav = await screen.findByRole("navigation", {
      name: "Settings sections",
    });
    const root = document.documentElement;
    const props = { scrollTop: 400, clientHeight: 800, scrollHeight: 1200 };
    for (const [key, value] of Object.entries(props)) {
      Object.defineProperty(root, key, { configurable: true, value });
    }
    try {
      act(() => {
        window.dispatchEvent(new Event("scroll"));
      });
      expect(
        within(nav).getByRole("link", { name: "Danger zone" }),
      ).toHaveAttribute("aria-current", "location");
    } finally {
      for (const key of Object.keys(props)) {
        Reflect.deleteProperty(root, key);
      }
    }
  });

  it("leaves Backup and the Danger zone out of a non-admin's index", async () => {
    serve({}, "editor");
    renderApp("/d/eng/settings");
    const nav = await screen.findByRole("navigation", {
      name: "Settings sections",
    });
    const labels = within(nav)
      .getAllByRole("link")
      .map((link) => link.textContent);
    expect(labels).not.toContain("Backup");
    expect(labels).not.toContain("Danger zone");
    expect(labels).toContain("Members");
  });

  it("works without an IntersectionObserver: the first section is current", async () => {
    vi.stubGlobal("IntersectionObserver", undefined);
    try {
      serve();
      renderApp("/d/eng/settings");
      const nav = await screen.findByRole("navigation", {
        name: "Settings sections",
      });
      expect(
        within(nav).getByRole("link", { name: "Members" }),
      ).toHaveAttribute("aria-current", "location");
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("scrolls from the phone select and sets the hash", async () => {
    const spy = vi.fn();
    Element.prototype.scrollIntoView = spy;
    let hash = "";
    function Probe() {
      hash = useLocation().hash;
      return null;
    }
    try {
      serve();
      render(
        <MemoryRouter initialEntries={["/d/eng/settings"]}>
          <App />
          <Probe />
        </MemoryRouter>,
      );
      const jump = await screen.findByRole("combobox", {
        name: "Jump to a section",
      });
      spy.mockClear();
      await userEvent.selectOptions(jump, "backup");
      expect(hash).toBe("#backup");
      expect(spy.mock.contexts).toContain(document.getElementById("backup"));
    } finally {
      Reflect.deleteProperty(Element.prototype, "scrollIntoView");
    }
  });

  it("offers the same sections as a select for a phone, which jumps to one", async () => {
    serve();

    renderApp("/d/eng/settings");

    const jump = await screen.findByRole("combobox", {
      name: "Jump to a section",
    });
    expect(
      within(jump)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual([
      "Members",
      "Review mode",
      "Domain policies",
      "Provisioning",
      "Tag aliases",
      "Backup",
      "Danger zone",
    ]);
    // Drawn for narrow widths only, and the rail for wide ones: the classes
    // are the layout, so they are what is pinned here.
    expect(jump.parentElement).toHaveClass("lg:hidden");
    expect(
      screen.getByRole("navigation", { name: "Settings sections" }),
    ).toHaveClass("hidden", "lg:block");
    await userEvent.selectOptions(jump, "backup");
    expect(jump).toHaveValue("backup");
  });

  it("draws no members card for a caller the members read refuses", async () => {
    serve({
      "/domains/eng/members": () => {
        throw new ApiProblem(403, "forbidden", "not a member");
      },
    });

    renderApp("/d/eng/settings");

    await screen.findByRole("region", { name: "Domain policies" });
    await waitFor(() => {
      expect(
        apiMock.mock.calls.some((call) => call[0] === "/domains/eng/members"),
      ).toBe(true);
    });
    expect(screen.queryByRole("region", { name: "Members" })).toBeNull();
  });

  it("explains a configuration section that is not declared, and offers to start it", async () => {
    serve();

    renderApp("/d/eng/settings");

    const aliases = await screen.findByRole("region", { name: "Tag aliases" });
    // The meaning and the syntax, both the server's.
    expect(
      within(aliases).getByText(/Spellings that fold into one canonical tag/),
    ).toBeVisible();
    expect(within(aliases).getByText(/k8s -> kubernetes/)).toBeVisible();
    expect(
      within(aliases).getByRole("button", { name: "Add Tag Aliases" }),
    ).toBeVisible();
    expect(
      within(screen.getByRole("region", { name: "Provisioning" })).getByRole(
        "button",
        { name: "Add Provisioning" },
      ),
    ).toBeVisible();
  });

  it("says nothing extra about a section that is declared, and lists its problems", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: MANIFEST,
        sections: manifestSectionsResponse({
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

    renderApp("/d/eng/settings");

    const aliases = await screen.findByRole("region", { name: "Tag aliases" });
    // A chained alias is a declaration AND a problem, so the bullet stands
    // twice: once in the list, once above the reason it was flagged for.
    expect(within(aliases).getAllByText("foo -> bar")).toHaveLength(2);
    expect(
      within(aliases).getByText(
        "canonical `bar` is itself an alias, resolution stays a single hop",
      ),
    ).toBeVisible();
    expect(
      within(aliases).queryByText(/Spellings that fold into one canonical tag/),
    ).toBeNull();
    expect(
      within(aliases).queryByRole("button", { name: "Add Tag Aliases" }),
    ).toBeNull();
  });

  it("shows the explanation but no action to a reader who cannot edit", async () => {
    serve({}, "editor");

    renderApp("/d/eng/settings");

    const aliases = await screen.findByRole("region", { name: "Tag aliases" });
    expect(
      within(aliases).getByText(/Spellings that fold into one canonical tag/),
    ).toBeVisible();
    expect(
      within(aliases).queryByRole("button", { name: "Add Tag Aliases" }),
    ).toBeNull();
  });

  it("draws the old bare line for a daemon that sends no startable sections", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: MANIFEST,
        sections: manifestSectionsResponse({ starters: [] }),
      }),
    });

    renderApp("/d/eng/settings");

    const aliases = await screen.findByRole("region", { name: "Tag aliases" });
    expect(within(aliases).getByText("No aliases")).toBeVisible();
    expect(
      within(aliases).queryByRole("button", { name: "Add Tag Aliases" }),
    ).toBeNull();
  });

  it("carries the section to the editor when the action is pressed", async () => {
    serve({ "/validate": () => ({ findings: [], errors: 0 }) });

    renderApp("/d/eng/settings");

    await userEvent.click(
      await screen.findByRole("button", { name: "Add Tag Aliases" }),
    );

    expect(
      await screen.findByRole("heading", { name: "Editing eng MANIFEST" }),
    ).toBeVisible();
    const editor = await screen.findByLabelText("MANIFEST source");
    await waitFor(() => {
      expect(editor.textContent).toContain("## Tag Aliases");
    });
    expect(editor.textContent).toContain("k8s -> kubernetes");
  });

  it("arrives armed from the front page's Unregister domain palette row, at the question and focused", async () => {
    serve();
    const user = userEvent.setup();

    renderApp("/d/eng");
    await screen.findByRole("article", { name: "MANIFEST" });
    await user.keyboard("{Meta>}k{/Meta}");
    await user.click(
      await screen.findByRole("option", { name: /unregister domain/i }),
    );

    // The row asks, it does not act: the question waits in the danger zone,
    // in view and with the typed field focused.
    const card = await screen.findByRole("region", { name: "Danger zone" });
    const field = await within(card).findByLabelText("Type eng to confirm");
    await waitFor(() => {
      expect(field).toHaveFocus();
    });
    expect(card.closest("#danger-zone")).not.toBeNull();
  });

  it("clears the arming from the history entry, so a reload or a step back does not ask again", async () => {
    serve();
    let seen: unknown = "unset";
    function Probe() {
      seen = useLocation().state;
      return null;
    }

    render(
      <MemoryRouter
        initialEntries={[
          {
            pathname: "/d/eng/settings",
            hash: "#danger-zone",
            state: { unregister: true },
          },
        ]}
      >
        <App />
        <Probe />
      </MemoryRouter>,
    );

    const card = await screen.findByRole("region", { name: "Danger zone" });
    expect(
      await within(card).findByLabelText("Type eng to confirm"),
    ).toBeVisible();
    // Replaced in place: same address, block kept, the arming gone.
    await waitFor(() => {
      expect(seen).toBeNull();
    });
  });

  it("answers an unknown domain the way the front page does", async () => {
    serve({
      "/domains/eng/tree": () => {
        throw new ApiProblem(404, "not found", "no domain");
      },
      "/reflection/domains": () => ({ domains: [] }),
    });

    renderApp("/d/eng/settings");

    expect(
      await screen.findByRole("heading", { name: "Domain not found" }),
    ).toBeVisible();
  });

  it("never flashes a card before an unknown domain's 404 lands", async () => {
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    serve(
      {
        "/domains/eng/tree": async () => {
          await held;
          throw new ApiProblem(404, "not found", "no domain");
        },
        "/reflection/domains": () => ({ domains: [] }),
      },
      "admin",
    );

    renderApp("/d/eng/settings");

    expect(
      await screen.findByRole("status", { name: "Loading the settings" }),
    ).toBeInTheDocument();
    // Every other read has answered by now; the tree has not.
    await waitFor(() => {
      expect(
        apiMock.mock.calls.some((call) => call[0] === "/domains/eng/members"),
      ).toBe(true);
    });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.queryByRole("region", { name: "Backup" })).toBeNull();
    expect(screen.queryByRole("region", { name: "Danger zone" })).toBeNull();
    release();
    expect(
      await screen.findByRole("heading", { name: "Domain not found" }),
    ).toBeVisible();
  });

  it("says a refused MANIFEST read in the server's words", async () => {
    serve({
      "/domains/eng/manifest": () => {
        throw new ApiProblem(
          403,
          "forbidden",
          "you may not read this MANIFEST",
        );
      },
    });

    renderApp("/d/eng/settings");

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "you may not read this MANIFEST",
    );
    expect(
      screen.queryByRole("region", { name: "Domain policies" }),
    ).toBeNull();
  });

  it("waits behind one loading state until its reads have answered", async () => {
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    serve({
      "/domains/eng/members": async () => {
        await held;
        return { owner: null, visibility: "shared", members: [] };
      },
    });

    renderApp("/d/eng/settings");

    expect(
      await screen.findByRole("status", { name: "Loading the settings" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("region", { name: "Domain policies" }),
    ).toBeNull();
    release();
    expect(
      await screen.findByRole("region", { name: "Members" }),
    ).toBeVisible();
  });

  it("opens a rail link at its block once the screen has loaded, also when members is refused", async () => {
    const scrolled: string[] = [];
    const spy = vi
      .spyOn(Element.prototype, "scrollIntoView")
      .mockImplementation(function (this: Element) {
        scrolled.push(this.id);
      });
    try {
      serve({
        "/domains/eng/members": () => {
          throw new ApiProblem(403, "forbidden", "not a member");
        },
      });
      renderApp("/d/eng/settings#policies");
      await screen.findByRole("region", { name: "Domain policies" });
      await waitFor(() => {
        expect(scrolled).toContain("policies");
      });
    } finally {
      spy.mockRestore();
    }
  });

  it("shuts unregistering and the visibility switch while a rename runs", async () => {
    serve({
      "/domains": () => ({
        ...domainsResponse(),
        domains: [{ ...domainsResponse().domains[0], renaming: true }],
      }),
    });

    renderApp("/d/eng/settings");

    const REASON = "A rename of this domain is already running.";
    const unregister = await screen.findByRole("button", {
      name: "Unregister domain",
    });
    // The listing that says a rename runs lands after the card is drawn.
    await waitFor(() => {
      expect(unregister).toHaveAttribute("aria-disabled", "true");
    });
    expect(unregister).toHaveAccessibleDescription(REASON);
    const share = await screen.findByRole("button", { name: "Make private" });
    expect(share).toHaveAttribute("aria-disabled", "true");
    expect(share).toHaveAccessibleDescription(REASON);
    const importArchive = screen.getByRole("button", {
      name: "Import archive",
    });
    expect(importArchive).toHaveAttribute("aria-disabled", "true");
    expect(importArchive).toHaveAccessibleDescription(REASON);
  });
});

describe("the domain settings kept texts section", () => {
  function keptEntry(overrides: Record<string, unknown> = {}) {
    return {
      id: "k1",
      draft: false,
      kind: "engram",
      path: "guides/a.md",
      kept_at: "2026-10-15T08:00:00Z",
      bytes: 12,
      sender: null,
      ...overrides,
    };
  }

  /** The kept routes over `serve`'s, with every drop recorded. */
  function serveKept(entries: ReturnType<typeof keptEntry>[]) {
    let listed = entries;
    const dropped: string[] = [];
    const texts: Record<string, string> = {
      k1: "# A\n\nThe room's text.\n",
      c1: "Please add the rollback step.",
    };
    const routes: Record<string, Answer> = {
      "/domains/eng/kept": () => ({ domain: "eng", kept: listed }),
    };
    for (const entry of entries) {
      routes[`/domains/eng/kept/${entry.id}`] = (_path, init) => {
        if (init?.method === "DELETE") {
          dropped.push(entry.id);
          listed = listed.filter((row) => row.id !== entry.id);
          return undefined;
        }
        return { domain: "eng", ...entry, text: texts[entry.id] ?? "" };
      };
    }
    serve(routes);
    return dropped;
  }

  it("lists each kept text with its page, kind, sender and time, newest first", async () => {
    serveKept([
      keptEntry(),
      keptEntry({
        id: "c1",
        kind: "comment",
        path: "guides/deploying.md",
        kept_at: "2026-10-16T09:30:00Z",
        sender: "ben",
      }),
    ]);
    renderApp("/d/eng/settings");
    const section = await screen.findByRole("region", { name: "Kept texts" });
    // In the index, between the configuration sections and Backup.
    expect(
      within(
        screen.getByRole("navigation", { name: "Settings sections" }),
      ).getByRole("link", { name: "Kept texts" }),
    ).toHaveAttribute("href", "/d/eng/settings#kept-texts");
    expect(
      precedes(screen.getByRole("region", { name: "Tag aliases" }), section),
    ).toBe(true);
    expect(
      precedes(section, screen.getByRole("region", { name: "Backup" })),
    ).toBe(true);
    const rows = within(
      within(section).getByRole("list", { name: "Kept texts" }),
    ).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent("guides/deploying.md");
    expect(rows[0]).toHaveTextContent("Comment");
    expect(rows[0]).toHaveTextContent("From ben");
    expect(rows[1]).toHaveTextContent("guides/a.md");
    expect(rows[1]).toHaveTextContent("Engram text");
    expect(rows[1]).not.toHaveTextContent("From");
  });

  it("shows a comment's text", async () => {
    serveKept([
      keptEntry({
        id: "c1",
        kind: "comment",
        path: "guides/deploying.md",
        sender: "ben",
      }),
    ]);
    renderApp("/d/eng/settings");
    const section = await screen.findByRole("region", { name: "Kept texts" });
    await userEvent.click(
      within(section).getByRole("button", {
        name: "Show the kept comment for guides/deploying.md",
      }),
    );
    const dialog = await screen.findByRole("dialog", { name: "Kept comment" });
    expect(dialog).toHaveTextContent("From ben");
    expect(
      await within(dialog).findByLabelText("The kept comment"),
    ).toHaveTextContent("Please add the rollback step.");
  });

  it("offers a kept file as a download, not as a text to show", async () => {
    serveKept([
      keptEntry({
        id: "f1",
        draft: true,
        path: "assets/deck.pdf",
        file: true,
      }),
    ]);
    renderApp("/d/eng/settings");
    const section = await screen.findByRole("region", { name: "Kept texts" });

    // A link the browser saves from, at the route that serves the bytes;
    // there is no text to show.
    const download = within(section).getByRole("link", {
      name: "Download the kept file for assets/deck.pdf",
    });
    expect(download).toHaveAttribute(
      "href",
      "/api/v1/domains/eng/kept/f1/file",
    );
    expect(download).toHaveAttribute("download");
    expect(
      within(section).queryByRole("button", { name: /^Show the kept/ }),
    ).toBeNull();
    expect(
      within(section).getByRole("button", {
        name: "Drop the kept file for assets/deck.pdf",
      }),
    ).toBeVisible();
  });

  it("drops a kept text after a confirm, then says none are left", async () => {
    const dropped = serveKept([keptEntry()]);
    renderApp("/d/eng/settings");
    const section = await screen.findByRole("region", { name: "Kept texts" });
    await userEvent.click(
      within(section).getByRole("button", {
        name: "Drop the kept text for guides/a.md",
      }),
    );
    expect(dropped).toEqual([]);
    await userEvent.click(
      within(section).getByRole("button", { name: "Drop it" }),
    );
    await waitFor(() => {
      expect(dropped).toEqual(["k1"]);
    });
    expect(await within(section).findByText("No kept texts.")).toBeVisible();
  });

  it("shows Drop shut on a read-only instance", async () => {
    serveKept([keptEntry()]);
    const routes = apiMock.getMockImplementation();
    apiMock.mockImplementation((path, init) =>
      path === "/auth/me"
        ? Promise.resolve(
            meResponse({ user: userFixture(), read_only: true }) as never,
          )
        : (routes as NonNullable<typeof routes>)(path, init),
    );
    renderApp("/d/eng/settings");
    const section = await screen.findByRole("region", { name: "Kept texts" });
    expect(
      within(section).getByRole("button", {
        name: "Drop the kept text for guides/a.md",
      }),
    ).toHaveAttribute("aria-disabled", "true");
    expect(
      within(section).getByRole("button", {
        name: "Show the kept text for guides/a.md",
      }),
    ).toBeEnabled();
  });

  it("says so when nothing is kept", async () => {
    serveKept([]);
    renderApp("/d/eng/settings");
    const section = await screen.findByRole("region", { name: "Kept texts" });
    expect(within(section).getByText("No kept texts.")).toBeVisible();
    expect(within(section).queryByRole("list")).toBeNull();
  });

  it("is not drawn for a person with no right to kept texts", async () => {
    serve({
      "/domains/eng/kept": () => {
        throw new ApiProblem(403, "forbidden", "kept texts need write access");
      },
    });
    renderApp("/d/eng/settings");
    await screen.findByRole("region", { name: "Backup" });
    expect(screen.queryByRole("region", { name: "Kept texts" })).toBeNull();
    expect(
      screen.queryByText("kept texts need write access"),
    ).not.toBeInTheDocument();
  });

  it("says why when the listing fails for another reason", async () => {
    serve({
      "/domains/eng/kept": () => {
        throw new ApiProblem(500, "internal", "the state folder is unreadable");
      },
    });
    renderApp("/d/eng/settings");
    const section = await screen.findByRole("region", { name: "Kept texts" });
    expect(within(section).getByRole("alert")).toHaveTextContent(
      "the state folder is unreadable",
    );
  });
});
