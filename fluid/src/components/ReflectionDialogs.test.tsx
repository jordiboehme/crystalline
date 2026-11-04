import { QueryClient } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useEffect } from "react";
import { MemoryRouter, useLocation, useNavigate } from "react-router";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import App from "../App";
import {
  REFLECTION_DOMAINS_KEY,
  invalidateReflection,
  reflectionKey,
  reflectionStatusKey,
} from "../api/reflectionDomains";
import {
  clearDraft,
  readDraft,
  readOffers,
  writeDraft,
} from "../editor/drafts";
import { reflectionOfferKey, reflectionRoomKey } from "../collab/provider";

import { ApiProblem, api } from "../api/client";
import type { Answer } from "../test/harness";
import {
  answersFor,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";
import { newPagePath } from "./NewReflectionDialog";

vi.setConfig({ testTimeout: 15000 });
vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});
const apiMock = vi.mocked(api);

function serve(right: string, routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "ann", role: "viewer" }) }),
      "/domains": () => ({ behavior: [], domains: [] }),
      "/reflection/domains": () => ({ domains: [] }),
      "/domains/eng/reflection": () => ({
        domain: "eng",
        enabled: true,
        state: "on",
        home: "local",
        home_url: null,
        right,
        engrams_visible: false,
        pending_engrams: [],
      }),
      "/domains/eng/reflection/tree": () => ({
        domain: "eng",
        pact: { path: "PACT.md", title: "PACT", exists: true, open_threads: 0 },
        nodes: [
          {
            kind: "reflection",
            name: "a.md",
            path: "guides/a.md",
            title: "A",
            open_threads: 0,
            sections: 1,
          },
        ],
      }),
      "/domains/eng/reflections/guides/a.md": () => ({
        domain: "eng",
        path: "guides/a.md",
        title: "A",
        markdown: "# A\n",
        checksum: "c1",
        sections: [],
        threads: [],
      }),
      "/domains/eng/reflections/PACT.md": () => ({
        domain: "eng",
        path: "PACT.md",
        title: "PACT",
        markdown: "# PACT\n",
        checksum: "c2",
        sections: [],
        threads: [],
      }),
      ...routes,
    }),
  );
}

function body(path: string, method: string): unknown {
  const call = apiMock.mock.calls.find(
    ([sent, init]) => sent === path && init?.method === method,
  );
  return call ? (JSON.parse(call[1]?.body as string) as unknown) : undefined;
}

beforeEach(() => {
  apiMock.mockReset();
});

// The editor's screen is a lazy part: its first import compiles the whole
// editor, which on a loaded machine takes longer than a `findByRole` waits
// (D37). It is loaded here, once, so a test that opens it waits for the
// screen, not for the compile.
beforeAll(async () => {
  await import("../screens/ReflectionEditor");
}, 120_000);

describe("newPagePath", () => {
  it("names a page after its title, in the folder given", () => {
    expect(newPagePath("guides", "Night Deploys")).toBe(
      "guides/night-deploys.md",
    );
    expect(newPagePath("", "Getting started!")).toBe("getting-started.md");
    expect(newPagePath("guides", "  ")).toBe("");
  });
});

describe("creating, moving and deleting pages", () => {
  it("creates a page without ever overwriting one, and opens it in the editor", async () => {
    serve("edit", {
      "/domains/eng/reflections/guides/night-deploys.md": (_p, init) =>
        init?.method === "PUT"
          ? { checksum: "n", created: true, ids_assigned: [] }
          : {
              domain: "eng",
              path: "guides/night-deploys.md",
              title: "Night Deploys",
              markdown: "# Night Deploys\n",
              checksum: "n",
              sections: [],
              threads: [],
            },
    });
    renderApp("/d/eng/reflection");
    await userEvent.click(
      await screen.findByRole("button", { name: "New page" }),
    );
    const dialog = await screen.findByRole("dialog", { name: "New page" });
    await userEvent.type(
      within(dialog).getByRole("textbox", { name: "Folder" }),
      "guides",
    );
    await userEvent.type(
      within(dialog).getByRole("textbox", { name: "Title" }),
      "Night Deploys",
    );
    expect(
      within(dialog).getByText(
        "The page will be saved as guides/night-deploys.md.",
      ),
    ).toBeVisible();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create page" }),
    );
    await waitFor(() => {
      expect(
        body("/domains/eng/reflections/guides/night-deploys.md", "PUT"),
      ).toEqual({
        markdown: "# Night Deploys\n",
        create: true,
      });
    });
    // The editor's own screen, whatever the room does in jsdom: it starts on
    // the connecting skeleton.
    expect(
      await screen.findByRole("status", { name: "Connecting the session" }),
    ).toBeVisible();
  });

  it("shows the server's words when the path is taken", async () => {
    serve("edit", {
      "/domains/eng/reflections/guides/a.md": (_p, init) => {
        if (init?.method === "PUT") {
          throw new ApiProblem(
            409,
            "conflict",
            "a reflection already lives at 'guides/a.md'",
          );
        }
        return {
          domain: "eng",
          path: "guides/a.md",
          title: "A",
          markdown: "# A\n",
          checksum: "c1",
          sections: [],
          threads: [],
        };
      },
    });
    renderApp("/d/eng/reflection");
    await userEvent.click(
      await screen.findByRole("button", { name: "New page" }),
    );
    const dialog = await screen.findByRole("dialog", { name: "New page" });
    await userEvent.type(
      within(dialog).getByRole("textbox", { name: "Folder" }),
      "guides",
    );
    await userEvent.type(
      within(dialog).getByRole("textbox", { name: "Title" }),
      "A",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create page" }),
    );
    expect(
      await within(dialog).findByText(
        "a reflection already lives at 'guides/a.md'",
      ),
    ).toBeVisible();
  });

  it("moves a page and follows it, and says when somebody is editing it", async () => {
    let refuse = true;
    serve("edit", {
      "/domains/eng/reflection/move": () => {
        if (refuse) {
          throw new ApiProblem(
            409,
            "conflict",
            "somebody is editing 'guides/a.md'; try again once the page is closed",
          );
        }
        return {
          from: "guides/a.md",
          to: "howto/a.md",
          links_rewritten: 1,
          threads_moved: true,
        };
      },
      "/domains/eng/reflections/howto/a.md": () => ({
        domain: "eng",
        path: "howto/a.md",
        title: "A",
        markdown: "# A\n",
        checksum: "c1",
        sections: [],
        threads: [],
      }),
    });
    renderApp("/d/eng/reflection/guides/a.md");
    await choose("Move");
    const dialog = await screen.findByRole("dialog", { name: "Move A" });
    const field = within(dialog).getByRole("textbox", { name: "New path" });
    await userEvent.clear(field);
    await userEvent.type(field, "howto/a.md");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Move page" }),
    );
    expect(
      await within(dialog).findByText(
        "somebody is editing 'guides/a.md'; try again once the page is closed",
      ),
    ).toBeVisible();
    refuse = false;
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Move page" }),
    );
    await waitFor(() => {
      expect(body("/domains/eng/reflection/move", "POST")).toEqual({
        from: "guides/a.md",
        to: "howto/a.md",
      });
    });
    // The trail follows the page to its new folder. Looked up afresh on every
    // try: the page shows a skeleton while the new address loads, and the
    // trail of the old page is gone by then.
    await waitFor(() => {
      expect(
        within(
          screen.getByRole("navigation", { name: "Breadcrumb" }),
        ).getByText("howto"),
      ).toBeVisible();
    });
  });

  it("lists the pages a move could not relink", async () => {
    serve("edit", {
      "/domains/eng/reflection/move": () => ({
        from: "guides/a.md",
        to: "howto/a.md",
        links_rewritten: 0,
        threads_moved: true,
        links_failed: [{ path: "guides/b.md", reason: "it is open in a room" }],
      }),
      "/domains/eng/reflections/howto/a.md": () => ({
        domain: "eng",
        path: "howto/a.md",
        title: "A",
        markdown: "# A\n",
        checksum: "c1",
        sections: [],
        threads: [],
      }),
    });
    renderApp("/d/eng/reflection/guides/a.md");
    await choose("Move");
    const dialog = await screen.findByRole("dialog", { name: "Move A" });
    const field = within(dialog).getByRole("textbox", { name: "New path" });
    await userEvent.clear(field);
    await userEvent.type(field, "howto/a.md");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Move page" }),
    );
    expect(await within(dialog).findByText("guides/b.md")).toBeVisible();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Close" }),
    );
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("deletes a page after a second press", async () => {
    const deleted = vi.fn(() => undefined);
    serve("edit", {
      "/domains/eng/reflections/guides/a.md": (_p, init) =>
        init?.method === "DELETE"
          ? deleted()
          : {
              domain: "eng",
              path: "guides/a.md",
              title: "A",
              markdown: "# A\n",
              checksum: "c1",
              sections: [],
              threads: [],
            },
    });
    renderApp("/d/eng/reflection/guides/a.md");
    await choose("Delete");
    await userEvent.click(
      screen.getByRole("button", { name: "Confirm delete" }),
    );
    await waitFor(() => {
      expect(deleted).toHaveBeenCalledTimes(1);
    });
    // Back on the Reflection home.
    expect(
      await screen.findByRole("searchbox", { name: "Search the Reflection" }),
    ).toBeVisible();
  });

  it("never offers to move or delete PACT.md, and offers none of the three to a commenter", async () => {
    serve("curate");
    renderApp("/d/eng/reflection/PACT.md");
    await screen.findByRole("link", { name: "Edit" });
    // Download and Print view only: the pact can be neither moved nor deleted.
    await userEvent.click(screen.getByRole("button", { name: "More actions" }));
    expect(
      (await screen.findAllByRole("menuitem")).map((row) => row.textContent),
    ).toEqual(["Download as Markdown", "Print view"]);
  });

  it("offers a commenter no New page, Move or Delete", async () => {
    serve("read");
    renderApp("/d/eng/reflection/guides/a.md");
    await screen.findByRole("heading", { name: "A" });
    expect(screen.queryByRole("button", { name: "New page" })).toBeNull();
    // Download and Print view only: nothing to move or delete for this reader.
    await userEvent.click(screen.getByRole("button", { name: "More actions" }));
    expect(
      (await screen.findAllByRole("menuitem")).map((row) => row.textContent),
    ).toEqual(["Download as Markdown", "Print view"]);
  });
});

/** Opens the page's More actions menu and picks one row. */
async function choose(name: string): Promise<void> {
  await userEvent.click(
    await screen.findByRole("button", { name: "More actions" }),
  );
  await userEvent.click(await screen.findByRole("menuitem", { name }));
}

function calls(path: string, method: string): number {
  return apiMock.mock.calls.filter(
    ([sent, init]) => sent === path && (init?.method ?? "GET") === method,
  ).length;
}

const DRAFT = { content: "# unsaved\n", baseChecksum: "c1", savedAt: "" };
const A = (path: string) => [reflectionRoomKey(path), reflectionOfferKey(path)];
const PAGE = (path: string, title = "A") => ({
  domain: "eng",
  path,
  title,
  markdown: `# ${title}\n`,
  checksum: "c1",
  sections: [],
  threads: [],
});

describe("invalidateReflection", () => {
  it("invalidates the Reflection, its status and the domains listing", () => {
    const client = new QueryClient();
    const spy = vi.spyOn(client, "invalidateQueries");
    invalidateReflection(client, "eng");
    expect(spy.mock.calls.map(([f]) => f?.queryKey)).toEqual([
      reflectionKey("eng"),
      reflectionStatusKey("eng"),
      REFLECTION_DOMAINS_KEY,
    ]);
  });
});

describe("after a create, move or delete", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("refreshes the status on a delete, and lands on the home with focus", async () => {
    serve("edit", {
      "/domains/eng/reflections/guides/a.md": (_p, init) =>
        init?.method === "DELETE" ? undefined : PAGE("guides/a.md"),
    });
    renderApp("/d/eng/reflection/guides/a.md");
    await choose("Delete");
    const status = calls("/domains/eng/reflection", "GET");
    await userEvent.click(
      screen.getByRole("button", { name: "Confirm delete" }),
    );
    await waitFor(() => {
      expect(calls("/domains/eng/reflection", "GET")).toBeGreaterThan(status);
      expect(document.activeElement).toBe(
        screen.getByRole("heading", { name: "eng", level: 1 }),
      );
    });
    // The deleted page is dropped, never refetched into a 404.
    expect(calls("/domains/eng/reflections/guides/a.md", "GET")).toBe(1);
  });

  it("removes both drafts of a deleted page and no others", async () => {
    for (const key of [...A("guides/a.md"), ...A("guides/b.md")]) {
      writeDraft("ann", "eng", key, DRAFT);
    }
    serve("edit", {
      "/domains/eng/reflections/guides/a.md": (_p, init) =>
        init?.method === "DELETE" ? undefined : PAGE("guides/a.md"),
    });
    renderApp("/d/eng/reflection/guides/a.md");
    await choose("Delete");
    await userEvent.click(
      screen.getByRole("button", { name: "Confirm delete" }),
    );
    await screen.findByRole("heading", { name: "eng", level: 1 });
    for (const key of A("guides/a.md")) {
      expect(readDraft("ann", "eng", key)).toBeNull();
    }
    for (const key of A("guides/b.md")) {
      expect(readDraft("ann", "eng", key)).not.toBeNull();
    }
  });

  async function move(to: string) {
    await choose("Move");
    const dialog = await screen.findByRole("dialog", { name: "Move A" });
    const field = within(dialog).getByRole("textbox", { name: "New path" });
    await userEvent.clear(field);
    await userEvent.type(field, to);
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Move page" }),
    );
  }
  const MOVED = {
    "/domains/eng/reflection/move": () => ({
      from: "guides/a.md",
      to: "howto/a.md",
      links_rewritten: 0,
      threads_moved: true,
    }),
    "/domains/eng/reflections/howto/a.md": () => PAGE("howto/a.md"),
  };

  it("moves both drafts to the new path and focuses the page heading", async () => {
    for (const key of A("guides/a.md")) {
      writeDraft("ann", "eng", key, DRAFT);
    }
    serve("edit", MOVED);
    renderApp("/d/eng/reflection/guides/a.md");
    await move("howto/a.md");
    await waitFor(() => {
      expect(document.activeElement).toBe(
        screen.getByRole("heading", { name: "A", level: 1 }),
      );
    });
    for (const key of A("howto/a.md")) {
      expect(readDraft("ann", "eng", key)).toEqual(DRAFT);
    }
    for (const key of A("guides/a.md")) {
      expect(readDraft("ann", "eng", key)).toBeNull();
    }
  });

  it("keeps a draft that already sits at the new path and offers the old one there", async () => {
    const other = { ...DRAFT, content: "# newer\n" };
    writeDraft("ann", "eng", reflectionRoomKey("guides/a.md"), DRAFT);
    writeDraft("ann", "eng", reflectionRoomKey("howto/a.md"), other);
    serve("edit", MOVED);
    renderApp("/d/eng/reflection/guides/a.md");
    await move("howto/a.md");
    await screen.findByRole("heading", { name: "A", level: 1 });
    expect(readDraft("ann", "eng", reflectionRoomKey("howto/a.md"))).toEqual(
      other,
    );
    // Nothing is left at a path that no longer exists: the old copy is
    // offered at the new one.
    expect(
      readDraft("ann", "eng", reflectionRoomKey("guides/a.md")),
    ).toBeNull();
    expect(readOffers("ann", "eng", reflectionOfferKey("howto/a.md"))).toEqual([
      DRAFT,
    ]);
    clearDraft("ann", "eng", reflectionRoomKey("howto/a.md"));
    clearDraft("ann", "eng", reflectionOfferKey("howto/a.md"));
  });

  it("refreshes the status on a move", async () => {
    serve("edit", MOVED);
    renderApp("/d/eng/reflection/guides/a.md");
    await screen.findByRole("button", { name: "More actions" });
    const status = calls("/domains/eng/reflection", "GET");
    await move("howto/a.md");
    await waitFor(() => {
      expect(calls("/domains/eng/reflection", "GET")).toBeGreaterThan(status);
    });
  });

  it("focuses Close when pages could not be relinked, and the heading after it", async () => {
    serve("edit", {
      ...MOVED,
      "/domains/eng/reflection/move": () => ({
        from: "guides/a.md",
        to: "howto/a.md",
        links_rewritten: 0,
        threads_moved: true,
        links_failed: [{ path: "guides/b.md", reason: "it is open in a room" }],
      }),
    });
    renderApp("/d/eng/reflection/guides/a.md");
    await move("howto/a.md");
    const close = await screen.findByRole("button", { name: "Close" });
    await waitFor(() => {
      expect(document.activeElement).toBe(close);
    });
    await userEvent.click(close);
    await waitFor(() => {
      expect(document.activeElement).toBe(
        screen.getByRole("heading", { name: "A", level: 1 }),
      );
    });
  });

  it("focuses the heading after Close even when the moved page is drawn only after it", async () => {
    // The page at the new path answers only once Close was pressed: on a
    // loaded machine its read can lose that race, and the heading it draws
    // must still take focus when it comes.
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    serve("edit", {
      ...MOVED,
      "/domains/eng/reflection/move": () => ({
        from: "guides/a.md",
        to: "howto/a.md",
        links_rewritten: 0,
        threads_moved: true,
        links_failed: [{ path: "guides/b.md", reason: "it is open in a room" }],
      }),
      "/domains/eng/reflections/howto/a.md": async () => {
        await held;
        return PAGE("howto/a.md");
      },
    });
    renderApp("/d/eng/reflection/guides/a.md");
    await move("howto/a.md");
    const close = await screen.findByRole("button", { name: "Close" });
    await waitFor(() => {
      expect(document.activeElement).toBe(close);
    });
    await userEvent.click(close);
    expect(screen.queryByRole("heading", { name: "A", level: 1 })).toBeNull();
    release();
    const heading = await screen.findByRole("heading", { name: "A", level: 1 });
    await waitFor(() => {
      expect(document.activeElement).toBe(heading);
    });
  });

  it("closes the move dialog when the route changes under it", async () => {
    let go: (to: string) => void = () => undefined;
    function Capture() {
      const navigate = useNavigate();
      useEffect(() => {
        go = (to) => void navigate(to);
      }, [navigate]);
      return <output data-testid="at">{useLocation().pathname}</output>;
    }
    serve("edit", {
      "/domains/eng/reflection/move": () => ({
        from: "guides/a.md",
        to: "howto/a.md",
        links_rewritten: 0,
        threads_moved: true,
        links_failed: [{ path: "guides/b.md", reason: "x" }],
      }),
      "/domains/eng/reflections/howto/a.md": () => PAGE("howto/a.md"),
      "/domains/eng/reflections/guides/a.md": () => PAGE("guides/a.md"),
    });
    render(
      <MemoryRouter initialEntries={["/d/eng/reflection/guides/a.md"]}>
        <App />
        <Capture />
      </MemoryRouter>,
    );
    await move("howto/a.md");
    await screen.findByRole("button", { name: "Close" });
    // The move's own navigation commits as a transition, so it can land after
    // the dialog shows its receipt. Going back before it has would leave the
    // page at the address it never left, and nothing would close.
    await waitFor(() => {
      expect(screen.getByTestId("at")).toHaveTextContent(
        "/d/eng/reflection/howto/a.md",
      );
    });
    act(() => {
      go("/d/eng/reflection/guides/a.md");
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    // The dialog's unmount runs its close handler a task later; a route
    // change is not the person pressing Close, so nothing sends them back.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
    });
    expect(screen.getByTestId("at")).toHaveTextContent(
      "/d/eng/reflection/guides/a.md",
    );
  });
});

describe("dialog details", () => {
  it("disables Move page while the path is empty or unchanged", async () => {
    serve("edit");
    renderApp("/d/eng/reflection/guides/a.md");
    await choose("Move");
    const dialog = await screen.findByRole("dialog", { name: "Move A" });
    const button = within(dialog).getByRole("button", { name: "Move page" });
    expect(button).toBeDisabled();
    const field = within(dialog).getByRole("textbox", { name: "New path" });
    await userEvent.clear(field);
    expect(button).toBeDisabled();
    await userEvent.type(field, "howto/a.md");
    expect(button).toBeEnabled();
  });

  it("says what a title needs when it makes no path", async () => {
    serve("edit");
    renderApp("/d/eng/reflection");
    await userEvent.click(
      await screen.findByRole("button", { name: "New page" }),
    );
    const dialog = await screen.findByRole("dialog", { name: "New page" });
    expect(within(dialog).getByText("Give the page a title.")).toBeVisible();
    await userEvent.type(
      within(dialog).getByRole("textbox", { name: "Title" }),
      "!!!",
    );
    expect(
      within(dialog).getByText(
        "The title needs letters or digits from a to z.",
      ),
    ).toBeVisible();
  });
});
