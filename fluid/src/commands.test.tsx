/**
 * The palette's actions, and the map of the keys that reach them.
 *
 * What is pinned here is the promise the keyboard makes: whatever a screen
 * lets you do with a pointer is on the palette while that screen is on, it is
 * gone the moment the screen is, and it is offered only to a session allowed
 * to do it. The help overlay is the other half of the promise - a shortcut
 * nobody is told about is a shortcut nobody presses - and it stays out of the
 * way of somebody typing a question mark into a field.
 */

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "./api/client";
import type { Role } from "./api/model";
import { useCollabSession } from "./collab/useCollabSession";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  soloCollabSession,
  userFixture,
} from "./test/harness";

// Above roughly load average 33 this file's slower tests exceed the 5000 ms
// default (a threshold effect measured 2026-08-14, plans history); the raise
// keeps a loaded machine from reading as a failure. Never raise the global
// default to hide this.
vi.setConfig({ testTimeout: 15000 });

vi.mock("./api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

// The palette is what this file is about, not co-editing: the editor route it
// visits opens on its solo surface, the way it does with no session server.
vi.mock("./collab/useCollabSession", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("./collab/useCollabSession")>();
  return { ...actual, useCollabSession: vi.fn() };
});

const apiMock = vi.mocked(api);
const collabMock = vi.mocked(useCollabSession);

const CONTENT = "---\ntitle: Alpha\n---\n\nA rule.\n";

/** The detail payload, in the engine's own shape. */
function detailResponse() {
  return {
    domain: "eng",
    permalink: "alpha",
    title: "Alpha",
    url: "crystalline://eng/alpha",
    path: "alpha.md",
    content: CONTENT,
    checksum: "3f8a1c05e2",
    frontmatter: { engram_type: "engram", status: "stable", tags: [] },
    observations: [],
    relations: [],
    links: [],
  };
}

/** Everything the engram screen and the editor behind it ask for. */
function serveEngramAs(
  role: Role,
  routes: Record<string, (path: string) => unknown> = {},
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role }) }),
      "/domains": domainsResponse,
      "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
      "/domains/eng/tree": () => ({ folders: [], engrams: [] }),
      "/domains/eng/manifest": () => ({ markdown: "# eng\n" }),
      "/domains/eng/engrams/alpha": () => detailResponse(),
      "/domains/eng/engrams": () => ({
        mode: "listing",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
      "/graph": () => ({ nodes: [], edges: [], truncated: false }),
      "/vocabulary": () => ({ tags: [], types: [], statuses: [] }),
      "/validate": () => ({ findings: [], errors: 0 }),
      ...routes,
    }),
  );
}

/** The common case: an editor, who may write. */
function serveEngram(routes: Record<string, (path: string) => unknown> = {}) {
  serveEngramAs("editor", routes);
}

beforeEach(() => {
  apiMock.mockReset();
  collabMock.mockReset();
  collabMock.mockReturnValue(soloCollabSession());
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the palette's actions", () => {
  it("a screen's actions appear in the palette and run on select", async () => {
    serveEngram();
    const user = userEvent.setup();

    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha" });
    await user.keyboard("{Meta>}k{/Meta}");

    expect(
      await screen.findByRole("option", { name: /edit engram/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("option", { name: /retire engram/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("option", { name: /move engram/i }),
    ).toBeInTheDocument();

    await user.click(screen.getByRole("option", { name: /edit engram/i }));

    // Ran, and the screen it acted on is gone, so its actions went with it.
    await waitFor(() => {
      expect(
        screen.queryByRole("option", { name: /edit engram/i }),
      ).not.toBeInTheDocument();
    });
    expect(await screen.findByLabelText("Engram source")).toBeInTheDocument();
  });

  it("a viewer sees no write actions", async () => {
    serveEngramAs("viewer");
    const user = userEvent.setup();

    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha" });
    await user.keyboard("{Meta>}k{/Meta}");
    await screen.findByPlaceholderText(/jump to a domain/i);

    expect(
      screen.queryByRole("option", { name: /edit engram/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: /retire engram/i }),
    ).not.toBeInTheDocument();
    // What a viewer may do is still offered: the gate is on writing, not on
    // the palette.
    expect(
      screen.getByRole("option", { name: /print this engram/i }),
    ).toBeInTheDocument();
  });

  it("offers the domain's own write from the domain screen", async () => {
    serveEngram();
    const user = userEvent.setup();

    renderApp("/d/eng");
    await screen.findByRole("heading", { name: "eng" });
    await user.keyboard("{Meta>}k{/Meta}");
    await user.click(
      await screen.findByRole("option", { name: /new engram/i }),
    );

    expect(
      await screen.findByRole("dialog", { name: /new engram/i }),
    ).toBeInTheDocument();
  });

  it("runs the very handler the button runs", async () => {
    serveEngram();
    // The utility three do not act themselves: they reach through a ref into
    // the handlers `EngramActions` published. A ref that stopped being filled
    // would turn all three into rows that quietly do nothing, and every
    // assertion about the rows existing would still pass, so one of them is
    // run end to end here.
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    const user = userEvent.setup();

    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha" });
    await user.keyboard("{Meta>}k{/Meta}");
    await user.click(
      await screen.findByRole("option", { name: /print this engram/i }),
    );

    expect(print).toHaveBeenCalledTimes(1);
  });

  it("shares through the same live region the button announces in", async () => {
    serveEngram();
    const user = userEvent.setup();
    // After `setup()`, which installs a clipboard stub of its own, and defined
    // rather than assigned because that stub is a getter-only property.
    const write = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: write },
    });

    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha" });
    await user.keyboard("{Meta>}k{/Meta}");
    await user.click(
      await screen.findByRole("option", { name: /share link to this engram/i }),
    );

    expect(write).toHaveBeenCalledWith(
      expect.stringContaining("/d/eng/e/alpha"),
    );
    // The one live region, which is the whole reason the palette reaches for
    // the button's handler instead of copying the three lines: a share that
    // copied silently would be a share nobody could tell had happened.
    expect(await screen.findByText("Link copied")).toBeInTheDocument();
  });

  it("offers no write on a domain that does not exist", async () => {
    serveEngram({
      "/domains/eng/tree": () => {
        throw new ApiProblem(404, "not found", "no domain named eng");
      },
    });
    const user = userEvent.setup();

    renderApp("/d/eng");
    await screen.findByRole("heading", { name: /domain not found/i });
    await user.keyboard("{Meta>}k{/Meta}");
    await screen.findByPlaceholderText(/jump to a domain/i);

    // The screen draws no "New engram" button on this branch and mounts no
    // dialog, so a palette row here would be a door that does not open.
    expect(
      screen.queryByRole("option", { name: /new engram/i }),
    ).not.toBeInTheDocument();
  });

  it("offers no MANIFEST edit on a read that failed", async () => {
    // A refusal rather than a server error, so the query layer treats it as an
    // answer and does not retry: what is under test is the registration, not
    // how long a retry takes.
    serveEngramAs("admin", {
      "/domains/eng/manifest": () => {
        throw new ApiProblem(403, "forbidden", "this instance refuses that");
      },
    });
    const user = userEvent.setup();

    renderApp("/d/eng/manifest");
    await screen.findByRole("alert");
    await user.keyboard("{Meta>}k{/Meta}");
    await screen.findByPlaceholderText(/jump to a domain/i);

    expect(
      screen.queryByRole("option", { name: /edit manifest/i }),
    ).not.toBeInTheDocument();
  });

  it("leaves the first domain the default where a screen offers nothing", async () => {
    serveEngram();
    const user = userEvent.setup();

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });
    await user.keyboard("{Meta>}k{/Meta}");

    // The frame's own row is chrome, identical on every screen, so it never
    // takes the top slot from the jumps: Cmd+K then Enter goes where it went
    // before there were actions at all.
    const domain = await screen.findByRole("option", { name: /eng/ });
    expect(domain).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByRole("option", { name: /keyboard shortcuts/i }),
    ).toHaveAttribute("aria-selected", "false");
  });

  it("highlights the screen's own action rather than the frame's", async () => {
    serveEngram();
    const user = userEvent.setup();

    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha" });
    await user.keyboard("{Meta>}k{/Meta}");

    // The frame registers "Keyboard shortcuts" on every screen, and a screen
    // registers what it can do here. Enter follows the highlight, so which of
    // the two leads is the difference between Enter editing the engram in
    // front of somebody and Enter opening a help sheet.
    const edit = await screen.findByRole("option", { name: /edit engram/i });
    expect(edit).toHaveAttribute("aria-selected", "true");
  });

  it("filters the actions with what is typed", async () => {
    serveEngram();
    const user = userEvent.setup();

    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha" });
    await user.keyboard("{Meta>}k{/Meta}");
    await user.type(await screen.findByRole("combobox"), "retire");

    expect(
      await screen.findByRole("option", { name: /retire engram/i }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: /move engram/i }),
    ).not.toBeInTheDocument();
  });
});

describe("the shortcut help", () => {
  it("? opens the shortcut help unless typing in a field", async () => {
    serveEngram();
    const user = userEvent.setup();

    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha" });
    await user.keyboard("?");

    const help = await screen.findByRole("dialog", {
      name: /keyboard shortcuts/i,
    });
    expect(help).toBeInTheDocument();
    expect(help).toHaveTextContent(/command palette/i);
  });

  it("is on the palette too, from any screen", async () => {
    serveEngram();
    const user = userEvent.setup();

    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha" });
    await user.keyboard("{Meta>}k{/Meta}");
    await user.click(
      await screen.findByRole("option", { name: /keyboard shortcuts/i }),
    );

    expect(
      await screen.findByRole("dialog", { name: /keyboard shortcuts/i }),
    ).toBeInTheDocument();
  });

  it("leaves a question mark typed into a field alone", async () => {
    serveEngram();
    const user = userEvent.setup();

    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha" });
    const box = screen.getByLabelText("Search");
    await user.click(box);
    await user.keyboard("?");

    expect(box).toHaveValue("?");
    expect(
      screen.queryByRole("dialog", { name: /keyboard shortcuts/i }),
    ).not.toBeInTheDocument();
  });

  it("closes on Escape", async () => {
    serveEngram();
    const user = userEvent.setup();

    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha" });
    await user.keyboard("?");
    await screen.findByRole("dialog", { name: /keyboard shortcuts/i });

    await user.keyboard("{Escape}");

    await waitFor(() => {
      expect(
        screen.queryByRole("dialog", { name: /keyboard shortcuts/i }),
      ).not.toBeInTheDocument();
    });
  });
});
