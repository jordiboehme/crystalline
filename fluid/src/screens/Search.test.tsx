/**
 * Search, which is the one screen whose whole state is its URL.
 *
 * Three things have to hold, and each is a way the screen could be quietly
 * wrong. A query is one request per pause rather than one per keystroke, and
 * the pause is what lands in the URL, so a result page is a link somebody can
 * send. A filter that arrives in that URL is already applied when the screen
 * opens, because a tag link is exactly that URL. And the mode the engine
 * actually ran is what is on screen: hybrid falls back to text where there is
 * nothing embedded to search, and a screen that showed the mode it asked for
 * would be telling the reader about a search that never happened.
 *
 * The screen is mounted on its own router rather than through the whole app,
 * because the URL is its output and a test has to be able to read it.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import type { RenderResult } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  MemoryRouter,
  Route,
  Routes,
  useLocation,
  useNavigate,
} from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import { answersFor, domainsResponse } from "../test/harness";
import Search from "./Search";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

/** One hit, in the engine's own shape. */
function hit(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    permalink: "alpha",
    title: "Alpha",
    snippet: "The rule of thumb here.",
    score: 0.8,
    engram_type: "engram",
    status: "stable",
    tags: ["eng"],
    kind: "engram",
    ...overrides,
  };
}

/** A page envelope around some hits. `mode` is the mode that ran. */
function page(hits: unknown[], mode = "text") {
  return {
    mode,
    total: hits.length,
    page: 1,
    limit: 50,
    count: hits.length,
    hits,
  };
}

function vocabularyResponse() {
  return {
    tags: [{ name: "eng", engrams: 3, observations: 5 }],
    categories: [],
    relation_types: [],
  };
}

function serve(routes: Record<string, (path: string) => unknown> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/domains": domainsResponse,
      "/vocabulary": vocabularyResponse,
      "/search": () => page([hit()]),
      ...routes,
    }),
  );
}

/** Every path the screen asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => call[0]);
}

/** Just the searches. */
function searches(): string[] {
  return requested().filter((path) => path.startsWith("/search"));
}

/**
 * The URL, and a way back.
 *
 * The screen's whole state is the URL, so a test that cannot read it is
 * testing something else. The back button is here for the same reason: which
 * changes are steps a reader can undo is part of the behavior.
 */
function Probe() {
  const location = useLocation();
  const navigate = useNavigate();
  return (
    <>
      <span data-testid="url">{`${location.pathname}${location.search}`}</span>
      <button
        type="button"
        onClick={() => {
          void navigate(-1);
        }}
      >
        Back
      </button>
    </>
  );
}

function url(): string {
  return screen.getByTestId("url").textContent ?? "";
}

function mount(entry = "/search"): RenderResult {
  // Retries off: a test that stubs a failure wants the failure, not a wait.
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[entry]}>
        <Routes>
          <Route path="/search" element={<Search />} />
        </Routes>
        <Probe />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the search screen", () => {
  it("searches once the typing pauses, and puts the query in the URL", async () => {
    serve();
    const user = userEvent.setup();

    mount();
    await user.type(screen.getByLabelText("Search query"), "rule");

    // Nothing has happened yet: not the URL, and not the server either.
    expect(url()).toBe("/search");
    expect(searches()).toEqual([]);

    await waitFor(() => {
      expect(url()).toBe("/search?q=rule");
    });
    expect(await screen.findByRole("link", { name: /Alpha/ })).toBeVisible();
    // One query for the pause, not one per keystroke.
    expect(searches()).toHaveLength(1);
    expect(searches()[0]).toContain("q=rule");
    expect(searches()[0]).not.toContain("q=rul&");
  });

  it("leaves the box alone while the URL it wrote settles", async () => {
    serve();
    const user = userEvent.setup();

    mount();
    // A word and the space before the next one. The URL is trimmed, the box
    // is not: a pause mid-sentence must not eat the space the reader typed.
    const box = screen.getByLabelText("Search query");
    await user.type(box, "rule ");

    await waitFor(() => {
      expect(url()).toBe("/search?q=rule");
    });
    expect(box).toHaveValue("rule ");
    // And it settles there rather than writing the same query over and over.
    await waitFor(() => {
      expect(searches()).toHaveLength(1);
    });
    expect(searches()).toHaveLength(1);
  });

  it("refires with the filter when a facet changes, and that is a step back", async () => {
    serve();
    const user = userEvent.setup();

    mount("/search?q=rule");
    await screen.findByRole("link", { name: /Alpha/ });

    await user.selectOptions(screen.getByLabelText("Mode"), "text");

    await waitFor(() => {
      expect(searches().some((path) => path.includes("search_type=text"))).toBe(
        true,
      );
    });
    expect(url()).toBe("/search?q=rule&search_type=text");

    // A facet is a deliberate move, so it is a history entry: the reader can
    // take it back without losing the query they typed.
    await user.click(screen.getByRole("button", { name: "Back" }));
    await waitFor(() => {
      expect(url()).toBe("/search?q=rule");
    });
  });

  it("narrows to a domain when its chip is turned on", async () => {
    serve();
    const user = userEvent.setup();

    mount("/search?q=rule");
    await screen.findByRole("link", { name: /Alpha/ });

    await user.click(screen.getByRole("button", { name: "eng" }));

    await waitFor(() => {
      expect(searches().some((path) => path.includes("domains=eng"))).toBe(
        true,
      );
    });
    expect(url()).toContain("domains=eng");
  });

  it("arrives with a tag from a link already applied", async () => {
    serve();

    mount("/search?tags=eng");

    // The chip a tag link points at is on when the screen opens.
    const chip = await screen.findByRole("button", { name: /#eng/ });
    expect(chip).toHaveAttribute("aria-pressed", "true");
    // And a filter with no query text is a search of its own, which the API
    // allows and this screen does not swallow.
    await waitFor(() => {
      expect(searches().some((path) => path.includes("tags=eng"))).toBe(true);
    });
    expect(await screen.findByRole("link", { name: /Alpha/ })).toBeVisible();
  });

  it("says which mode actually ran when it is not the one asked for", async () => {
    serve();

    mount("/search?q=rule");

    // Hybrid was asked for; with nothing embedded the engine ran text.
    expect(await screen.findByText(/ran as text/i)).toBeVisible();
  });

  it("claims no fallback when the mode asked for is the one that ran", async () => {
    serve({ "/search": () => page([hit()], "title") });

    mount("/search?q=rule&search_type=title");

    expect(await screen.findByRole("link", { name: /Alpha/ })).toBeVisible();
    expect(screen.queryByText(/fell back/i)).toBeNull();
  });

  it("does not search until there is something to search for", async () => {
    serve();

    mount();

    expect(
      await screen.findByText(/nothing has been searched for yet/i),
    ).toBeVisible();
    expect(searches()).toEqual([]);
  });

  it("says a filtered search that matched nothing matched nothing", async () => {
    serve({ "/search": () => page([]) });

    mount("/search?q=rule&tags=ghost");

    const empty = await screen.findByText(/no engram matches/i);
    expect(empty).toBeVisible();
    // Distinct from the screen nobody has typed into: this one names the
    // filters, because an empty answer under a filter is a normal answer.
    expect(empty).toHaveTextContent(/filter/i);
    expect(screen.queryByText(/nothing has been searched for yet/i)).toBeNull();
  });

  it("says how many and what ranked them on one line, not two", async () => {
    serve({ "/search": () => page([hit()], "title") });

    mount("/search?q=rule&search_type=title");

    expect(await screen.findByText("1 result, ranked by title.")).toBeVisible();
    // The count and the mode were two stacked lines saying near enough the
    // same thing; the tally line is gone.
    expect(screen.queryByText(/shown/)).toBeNull();
  });

  it("says what ranked an empty answer in a sentence of its own", async () => {
    // The ordinary empty search: the engine ran the mode that was asked for,
    // so there is no fallback to explain and no count to lead with. What is
    // left has to stand up on its own rather than trail off a missing tally.
    serve({ "/search": () => page([], "hybrid") });

    mount("/search?q=ghost");

    expect(await screen.findByText("Ranked by hybrid.")).toBeVisible();
    expect(screen.queryByText(/^ranked by/)).toBeNull();
    expect(screen.queryByText(/0 result/)).toBeNull();
  });

  it("offers a way out of an empty answer under a filter", async () => {
    serve({ "/search": () => page([]) });
    const user = userEvent.setup();

    mount("/search?q=rule&domains=eng&tags=ghost");

    await screen.findByText(/no engram matches/i);
    // Two ways out: widen to every domain, or drop the filters entirely. The
    // filter bar carries a clear of its own, so the recovery is the second.
    await user.click(
      screen.getByRole("button", { name: "Search all domains" }),
    );
    await waitFor(() => {
      expect(url()).not.toContain("domains=eng");
    });
    expect(url()).toContain("tags=ghost");

    const clears = screen.getAllByRole("button", { name: "Clear filters" });
    expect(clears).toHaveLength(2);
    await user.click(clears[1] as HTMLElement);
    await waitFor(() => {
      expect(url()).toBe("/search?q=rule");
    });
  });

  it("offers no way out when nothing was narrowed", async () => {
    serve({ "/search": () => page([]) });

    mount("/search?q=rule");

    await screen.findByText(/no engram matches/i);
    // Nothing is filtered, so a Clear filters button would do nothing at all.
    expect(screen.queryByRole("button", { name: "Clear filters" })).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Search all domains" }),
    ).toBeNull();
  });

  it("marks the searched words in a snippet without letting markup through", async () => {
    serve({
      "/search": () => page([hit({ snippet: "<b>rule</b> of thumb" })]),
    });

    mount("/search?q=rule");

    const marked = await screen.findAllByText("rule", { selector: "mark" });
    expect(marked).toHaveLength(1);
    // The snippet is text, not markup: the tags in it are shown as written
    // and never become elements.
    const row = screen.getByRole("link", { name: /Alpha/ });
    expect(row.querySelector("b")).toBeNull();
    expect(row.textContent).toContain("<b>rule</b>");
  });
});
