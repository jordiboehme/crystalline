import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation, useNavigate } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import type { Answer } from "../test/harness";
import {
  answersFor,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";
import { reflectionPathOf } from "../paths";
import { ReflectionNav } from "./ReflectionNav";

vi.setConfig({ testTimeout: 15000 });
vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});
const apiMock = vi.mocked(api);

/** A folder chain `l1/l2/.../lN` with one page at the bottom. */
function deepTree(depth: number) {
  const names = Array.from({ length: depth }, (_, i) => `l${String(i + 1)}`);
  let node: unknown = {
    kind: "reflection",
    name: "bottom.md",
    path: `${names.join("/")}/bottom.md`,
    title: "The bottom page",
    open_threads: 0,
    sections: 1,
  };
  for (let i = depth; i >= 1; i -= 1) {
    node = {
      kind: "folder",
      name: names[i - 1],
      path: names.slice(0, i).join("/"),
      children: [node],
    };
  }
  return {
    domain: "eng",
    pact: {
      path: "PACT.md",
      title: "How we write",
      exists: true,
      open_threads: 0,
    },
    nodes: [
      node,
      {
        kind: "reflection",
        name: "top.md",
        path: "top.md",
        title: "Top page",
        open_threads: 2,
        sections: 1,
      },
    ],
  };
}

function serve(routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "ann", role: "viewer" }) }),
      "/domains": () => ({ behavior: [], domains: [] }),
      "/reflection/domains": () => ({
        domains: [
          {
            domain: "eng",
            right: "edit",
            engrams_visible: false,
            home: "here",
            home_url: null,
            open_threads: 2,
          },
        ],
      }),
      "/domains/eng/reflection": () => ({
        domain: "eng",
        enabled: true,
        state: "on",
        home: "here",
        home_url: null,
        right: "edit",
        engrams_visible: false,
        pending_engrams: [],
      }),
      "/domains/eng/reflection/tree": () => deepTree(12),
      ...routes,
    }),
  );
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("reflectionPathOf", () => {
  it("reads the page path out of the domain splat", () => {
    expect(reflectionPathOf("reflection/guides/a.md")).toBe("guides/a.md");
    expect(reflectionPathOf("reflection-edit/guides/a.md")).toBe("guides/a.md");
    expect(reflectionPathOf("reflection")).toBe("");
    expect(reflectionPathOf("reflection/review")).toBe("");
    expect(reflectionPathOf("e/notes/a")).toBeNull();
    expect(reflectionPathOf("")).toBeNull();
  });
});

describe("the reflection tree", () => {
  it("a page twelve folders deep opens its whole path and is marked current", async () => {
    serve();
    const path = `${Array.from({ length: 12 }, (_, i) => `l${String(i + 1)}`).join("/")}/bottom.md`;
    renderApp(`/d/eng/reflection/${path}`);
    const nav = await screen.findByRole("navigation", {
      name: "Reflection of eng",
    });
    const current = await within(nav).findByRole("link", {
      name: "The bottom page",
    });
    expect(current).toHaveAttribute("aria-current", "page");
    expect(current).toHaveAttribute("title", "The bottom page");
    for (let i = 1; i <= 12; i += 1) {
      expect(
        within(nav).getByRole("button", { name: `Collapse l${String(i)}` }),
      ).toHaveAttribute("aria-expanded", "true");
    }
    // Indentation stops growing after six levels, so the name keeps its room.
    const indents = Array.from(nav.querySelectorAll("[data-depth]")).map((el) =>
      el.className.includes("border-l"),
    );
    expect(indents.filter(Boolean)).toHaveLength(6);
  });

  it("leads with Overview, How we write and Review, then the pages by title", async () => {
    serve();
    renderApp("/d/eng/reflection");
    const nav = await screen.findByRole("navigation", {
      name: "Reflection of eng",
    });
    // The tree arrives after the frame, so wait for the pact first.
    const pact = await within(nav).findByRole("link", { name: "How we write" });
    expect(pact).toHaveAttribute("href", "/d/eng/reflection/PACT.md");
    const links = within(nav).getAllByRole("link");
    // No repeated domain name: the top bar's switcher names the domain.
    expect(links.map((link) => link.textContent)).not.toContain("eng");
    expect(links[0]).toHaveAccessibleName("Overview");
    expect(links[0]).toHaveAttribute("href", "/d/eng/reflection");
    expect(links[0]).toHaveAttribute("aria-current", "page");
    expect(links[1]).toBe(pact);
    expect(links[2]).toHaveAccessibleName("Review");
    expect(links[2]).toHaveAttribute("href", "/d/eng/reflection/review");
    expect(
      within(nav).getByRole("heading", { level: 2, name: "Pages" }),
    ).toBeVisible();
    // Pages are named by their titles, never by their file names.
    const top = within(nav).getByRole("link", { name: "Top page" });
    expect(top).toHaveAttribute("href", "/d/eng/reflection/top.md");
    expect(within(nav).queryByText("top.md")).toBeNull();
    expect(within(nav).getByText("2 open threads")).toBeInTheDocument();
    expect(within(top).getByTitle("2 open threads")).toBeInTheDocument();
  });

  it("badges Review with the open threads of the whole tree", async () => {
    serve({
      "/domains/eng/reflection/tree": () => {
        const tree = deepTree(1);
        return { ...tree, pact: { ...tree.pact, open_threads: 1 } };
      },
    });
    renderApp("/d/eng/reflection/top.md");
    const nav = await screen.findByRole("navigation", {
      name: "Reflection of eng",
    });
    const review = await within(nav).findByRole("link", { name: "Review" });
    // Two on the top page and one on the pact: three, drawn in the row and
    // said in words beside it, so the link's name stays "Review".
    expect(review).toHaveTextContent("3");
    expect(within(review).getByTitle("3 open threads")).toBeInTheDocument();
    expect(within(nav).getByText("3 open threads in all")).toBeInTheDocument();
    expect(review).not.toHaveAttribute("aria-current");
    // Away from the home, Overview is not the current row.
    expect(
      within(nav).getByRole("link", { name: "Overview" }),
    ).not.toHaveAttribute("aria-current");
  });

  it("marks Review current on the review screen and draws no badge for none", async () => {
    serve({
      "/domains/eng/reflection/tree": () => ({
        ...deepTree(1),
        nodes: [],
      }),
      "/domains/eng/reflection/review": () => ({
        waiting_for_people: [],
        waiting_for_the_agent: [],
        queue: { total: 0, kinds: {}, queue: [] },
      }),
    });
    renderApp("/d/eng/reflection/review");
    const nav = await screen.findByRole("navigation", {
      name: "Reflection of eng",
    });
    const review = await within(nav).findByRole("link", { name: "Review" });
    expect(review).toHaveAttribute("aria-current", "page");
    await within(nav).findByRole("link", { name: "How we write" });
    expect(review).toHaveTextContent(/^Review$/);
  });

  it("offers a curator Write the pact while there is none, but not on the home", async () => {
    const noPact = () => {
      const tree = deepTree(1);
      return { ...tree, pact: { ...tree.pact, exists: false } };
    };
    const curator = {
      domain: "eng",
      enabled: true,
      state: "on",
      home: "here",
      home_url: null,
      right: "curate",
      engrams_visible: false,
      pending_engrams: [],
    };
    serve({
      "/domains/eng/reflection/tree": noPact,
      "/domains/eng/reflection": () => curator,
    });
    renderApp("/d/eng/reflection/top.md");
    const nav = await screen.findByRole("navigation", {
      name: "Reflection of eng",
    });
    expect(
      await within(nav).findByRole("button", { name: "Write the pact" }),
    ).toBeVisible();
    expect(
      within(nav).queryByRole("link", { name: "How we write" }),
    ).toBeNull();
  });

  it("leaves the pact row out on the home, where the home offers it", async () => {
    serve({
      "/domains/eng/reflection/tree": () => {
        const tree = deepTree(1);
        return { ...tree, pact: { ...tree.pact, exists: false } };
      },
      "/domains/eng/reflection": () => ({
        domain: "eng",
        enabled: true,
        state: "on",
        home: "here",
        home_url: null,
        right: "curate",
        engrams_visible: false,
        pending_engrams: [],
      }),
    });
    renderApp("/d/eng/reflection");
    const nav = await screen.findByRole("navigation", {
      name: "Reflection of eng",
    });
    await within(nav).findByRole("link", { name: "Top page" });
    expect(
      within(nav).queryByRole("button", { name: "Write the pact" }),
    ).toBeNull();
  });

  it("offers an editor no pact row at all while there is none", async () => {
    serve({
      "/domains/eng/reflection/tree": () => {
        const tree = deepTree(1);
        return { ...tree, pact: { ...tree.pact, exists: false } };
      },
    });
    renderApp("/d/eng/reflection/top.md");
    const nav = await screen.findByRole("navigation", {
      name: "Reflection of eng",
    });
    await within(nav).findByRole("link", { name: "Top page" });
    expect(
      within(nav).queryByRole("button", { name: "Write the pact" }),
    ).toBeNull();
    expect(
      within(nav).queryByRole("link", { name: "How we write" }),
    ).toBeNull();
    // New page is still offered to the editor.
    expect(within(nav).getByRole("button", { name: "New page" })).toBeVisible();
  });

  it("gives the keyboard back to New page when its dialog is dismissed", async () => {
    serve();
    renderApp("/d/eng/reflection/top.md");
    const nav = await screen.findByRole("navigation", {
      name: "Reflection of eng",
    });
    const user = userEvent.setup();
    const launcher = await within(nav).findByRole("button", {
      name: "New page",
    });
    await user.click(launcher);
    await screen.findByRole("dialog", { name: "New page" });
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    await waitFor(() => {
      expect(document.activeElement).toBe(launcher);
    });
  });

  it("folds a folder from its own button without leaving the page", async () => {
    serve();
    renderApp("/d/eng/reflection");
    const nav = await screen.findByRole("navigation", {
      name: "Reflection of eng",
    });
    await userEvent.click(
      await within(nav).findByRole("button", { name: "Expand l1" }),
    );
    expect(
      within(nav).getByRole("button", { name: "Collapse l1" }),
    ).toBeVisible();
    expect(
      within(nav).getByRole("button", { name: "Expand l2" }),
    ).toBeVisible();
  });

  it("opens the folders on the way when the reader walks to a page, and keeps the ones they opened", async () => {
    serve();
    const deep = `/d/eng/reflection/${Array.from({ length: 12 }, (_, i) => `l${String(i + 1)}`).join("/")}/bottom.md`;
    function Walk() {
      const navigate = useNavigate();
      const { pathname } = useLocation();
      return (
        <>
          <button type="button" onClick={() => void navigate(deep)}>
            walk
          </button>
          <ReflectionNav
            domain="eng"
            path={
              pathname === deep ? deep.slice("/d/eng/reflection/".length) : ""
            }
          />
        </>
      );
    }
    render(
      <QueryClientProvider client={new QueryClient()}>
        <MemoryRouter initialEntries={["/d/eng/reflection"]}>
          <Walk />
        </MemoryRouter>
      </QueryClientProvider>,
    );
    const nav = await screen.findByRole("navigation", {
      name: "Reflection of eng",
    });
    await userEvent.click(
      await within(nav).findByRole("button", { name: "Expand l1" }),
    );
    await userEvent.click(screen.getByRole("button", { name: "walk" }));
    expect(
      await within(nav).findByRole("button", { name: "Collapse l12" }),
    ).toBeVisible();
    expect(
      within(nav).getByRole("button", { name: "Collapse l1" }),
    ).toBeVisible();
  });
});

describe("retired pages in the tree", () => {
  const tree = () => ({
    domain: "eng",
    pact: { path: "PACT.md", title: "How we write", exists: true },
    nodes: [
      {
        kind: "folder",
        name: "old",
        path: "old",
        children: [
          {
            kind: "reflection",
            name: "gone.md",
            path: "old/gone.md",
            title: "Gone page",
            retired: true,
          },
        ],
      },
      {
        kind: "reflection",
        name: "kept.md",
        path: "kept.md",
        title: "Kept page",
      },
      {
        kind: "reflection",
        name: "past.md",
        path: "past.md",
        title: "Past page",
        retired: true,
      },
    ],
  });

  beforeEach(() => {
    localStorage.removeItem("fluid.reflection.retired");
  });

  it("hides retired pages and a folder they leave empty, until the switch shows them", async () => {
    serve({ "/domains/eng/reflection/tree": tree });
    renderApp("/d/eng/reflection");
    const nav = await screen.findByRole("navigation", {
      name: "Reflection of eng",
    });
    await within(nav).findByRole("link", { name: "Kept page" });
    expect(within(nav).queryByRole("link", { name: "Past page" })).toBeNull();
    expect(within(nav).queryByRole("button", { name: /old$/ })).toBeNull();
    const toggle = within(nav).getByRole("switch", {
      name: "Show retired pages",
    });
    expect(toggle).toHaveAttribute("aria-checked", "false");
    await userEvent.click(toggle);
    const past = within(nav).getByRole("link", { name: "Past page" });
    expect(past.className).toContain("italic");
    expect(
      within(nav).getByRole("button", { name: "Expand old" }),
    ).toBeVisible();
    // Remembered in this browser.
    expect(localStorage.getItem("fluid.reflection.retired")).toBe("shown");
  });

  it("keeps the retired page the reader is on", async () => {
    serve({ "/domains/eng/reflection/tree": tree });
    renderApp("/d/eng/reflection/past.md");
    const nav = await screen.findByRole("navigation", {
      name: "Reflection of eng",
    });
    expect(
      await within(nav).findByRole("link", { name: "Past page" }),
    ).toHaveAttribute("aria-current", "page");
  });

  it("offers no switch while no page is retired", async () => {
    serve();
    renderApp("/d/eng/reflection");
    const nav = await screen.findByRole("navigation", {
      name: "Reflection of eng",
    });
    await within(nav).findByRole("link", { name: "Top page" });
    expect(within(nav).queryByRole("switch")).toBeNull();
  });
});
