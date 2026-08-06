/**
 * The command palette, which is the keyboard's way around the app.
 *
 * What is pinned here is what makes it usable without a mouse and honest about
 * what it found. It opens on the shortcut a reader already knows from every
 * other tool, on either platform's modifier. It asks the server for titles
 * once the typing pauses rather than once per keystroke, and shows what came
 * back rather than a guess. Enter goes where the highlighted row says. Escape
 * puts the reader back exactly where they were, focus and all. And a query
 * that matched no title is never a dead end: the last row hands it to the
 * search screen, which searches the body text the palette never asked about.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
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

/** One title hit, in the engine's own shape. */
function hit(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    permalink: "notes/alpha",
    title: "Alpha",
    engram_type: "engram",
    status: "stable",
    tags: [],
    kind: "engram",
    ...overrides,
  };
}

/** A page envelope around some hits. */
function page(hits: unknown[]) {
  return {
    mode: "title",
    total: hits.length,
    page: 1,
    limit: 50,
    count: hits.length,
    hits,
  };
}

function serve(routes: Record<string, (path: string) => unknown> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
      "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
      "/search": () => page([hit()]),
      "/vocabulary": () => ({ tags: [] }),
      "/domains/eng/engrams/notes/alpha": () => ({
        domain: "eng",
        permalink: "notes/alpha",
        title: "Alpha",
        content: "Body.",
        frontmatter: { engram_type: "engram", status: "stable", tags: [] },
      }),
      "/graph": () => ({ nodes: [], edges: [], truncated: false }),
      "/domains/eng/engrams": () => page([]),
      "/domains/eng/tree": () => ({ folders: [], engrams: [] }),
      "/domains/eng/manifest": () => ({ markdown: "" }),
      ...routes,
    }),
  );
}

/** Just the searches the palette fired. */
function searches(): string[] {
  return apiMock.mock.calls
    .map((call) => call[0])
    .filter((path) => path.startsWith("/search"));
}

/** Open the app on the home screen, with the frame around it settled. */
async function openApp() {
  renderApp("/");
  await screen.findByRole("heading", { name: "Home" });
}

/** The palette's own dialog. */
function palette(): HTMLElement {
  return screen.getByRole("dialog", { name: /command palette/i });
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the command palette", () => {
  it("opens on Cmd+K", async () => {
    serve();
    const user = userEvent.setup();
    await openApp();

    await user.keyboard("{Meta>}k{/Meta}");

    expect(await screen.findByRole("dialog")).toBeVisible();
  });

  it("opens on Ctrl+K, for a keyboard without a Cmd key", async () => {
    serve();
    const user = userEvent.setup();
    await openApp();

    await user.keyboard("{Control>}k{/Control}");

    expect(await screen.findByRole("dialog")).toBeVisible();
  });

  it("asks for titles once the typing pauses, and lists what came back", async () => {
    serve();
    const user = userEvent.setup();
    await openApp();

    await user.keyboard("{Meta>}k{/Meta}");
    await user.type(await screen.findByRole("combobox"), "alph");

    await waitFor(() => {
      expect(searches()).toHaveLength(1);
    });
    // Titles only: the palette is a way to reach a thing whose name is half
    // remembered, and the search screen is where a body search belongs.
    expect(searches()[0]).toContain("search_type=title");
    expect(searches()[0]).toContain("q=alph");
    // One request for the pause rather than one per keystroke.
    expect(searches()).toHaveLength(1);
    expect(
      await within(palette()).findByRole("option", { name: /Alpha/ }),
    ).toBeVisible();
  });

  it("finds a title typed in whatever case the reader typed it in", async () => {
    serve();
    const user = userEvent.setup();
    await openApp();

    await user.keyboard("{Meta>}k{/Meta}");
    await user.type(await screen.findByRole("combobox"), "ALPH");

    // The domain rows match case insensitively, and the server does its own
    // matching: a reader holding shift is asking the same question.
    expect(
      await within(palette()).findByRole("option", { name: /Alpha/ }),
    ).toBeVisible();
  });

  it("goes to the engram Enter picks", async () => {
    serve();
    const user = userEvent.setup();
    await openApp();

    await user.keyboard("{Meta>}k{/Meta}");
    await user.type(await screen.findByRole("combobox"), "alph");
    await within(palette()).findByRole("option", { name: /Alpha/ });
    await user.keyboard("{Enter}");

    // The engram page, at the address the hit named: a permalink is a path,
    // and every segment of it survives the jump.
    expect(await screen.findByRole("heading", { name: "Alpha" })).toBeVisible();
    expect(apiMock.mock.calls.map((call) => call[0])).toContain(
      "/domains/eng/engrams/notes/alpha",
    );
  });

  it("walks the rows with the arrow keys, and Enter takes the one it lands on", async () => {
    serve();
    const user = userEvent.setup();
    await openApp();

    await user.keyboard("{Meta>}k{/Meta}");
    await user.type(await screen.findByRole("combobox"), "alph");
    const match = await within(palette()).findByRole("option", {
      name: /Alpha/,
    });
    const escape = within(palette()).getByRole("option", {
      name: /Search for/,
    });
    expect(match).toHaveAttribute("aria-selected", "true");

    await user.keyboard("{ArrowDown}");
    expect(escape).toHaveAttribute("aria-selected", "true");
    expect(match).toHaveAttribute("aria-selected", "false");

    await user.keyboard("{ArrowUp}");
    expect(match).toHaveAttribute("aria-selected", "true");

    // Enter follows the highlight rather than the top row, which is the whole
    // point of being able to move it.
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("heading", { name: "Alpha" })).toBeVisible();
  });

  it("forgets the query it jumped on, so the next Cmd+K opens clean", async () => {
    serve();
    const user = userEvent.setup();
    await openApp();

    await user.keyboard("{Meta>}k{/Meta}");
    await user.type(await screen.findByRole("combobox"), "alph");
    await within(palette()).findByRole("option", { name: /Alpha/ });
    await user.keyboard("{Enter}");
    await screen.findByRole("heading", { name: "Alpha" });
    const asked = searches().length;

    await user.keyboard("{Meta>}k{/Meta}");

    // A jump is how the palette is left most of the time, so it has to forget
    // on that path too: reopening onto the last query would answer the
    // question before this one.
    const box = await screen.findByRole("combobox");
    await waitFor(() => {
      expect(box).toHaveValue("");
    });
    // And it costs nothing on the wire either: no lookup for a term nobody is
    // asking about any more.
    expect(searches()).toHaveLength(asked);
  });

  it("goes to a domain the sidebar knows about", async () => {
    serve();
    const user = userEvent.setup();
    await openApp();

    await user.keyboard("{Meta>}k{/Meta}");
    await user.click(await screen.findByRole("option", { name: /eng/ }));

    expect(await screen.findByRole("heading", { name: "eng" })).toBeVisible();
  });

  it("hands a query it found no title for to the search screen", async () => {
    serve({ "/search": () => page([]) });
    const user = userEvent.setup();
    await openApp();

    await user.keyboard("{Meta>}k{/Meta}");
    await user.type(await screen.findByRole("combobox"), "salience");
    const escape = await within(palette()).findByRole("option", {
      name: /Search for/,
    });
    await user.click(escape);

    expect(
      await screen.findByRole("heading", { name: "Search" }),
    ).toBeVisible();
    await waitFor(() => {
      expect(screen.getByLabelText("Search query")).toHaveValue("salience");
    });
  });

  it("closes on Escape and gives the focus back to where it was", async () => {
    serve();
    const user = userEvent.setup();
    await openApp();

    const box = screen.getByLabelText("Search");
    box.focus();
    await user.keyboard("{Meta>}k{/Meta}");
    await screen.findByRole("dialog");

    await user.keyboard("{Escape}");

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    // Back where the reader was, not at the top of the document: a palette
    // that swallows the focus costs the keyboard reader their place.
    await waitFor(() => {
      expect(box).toHaveFocus();
    });
  });
});
