import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { Answer } from "../test/harness";
import {
  answersFor,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";
import type { ThreadView } from "../api/reflection";
import { ACTIVE_HIGHLIGHT, QUOTE_HIGHLIGHT } from "../reflectionQuotes";
import { waitingWords } from "./ReflectionThreads";

vi.setConfig({ testTimeout: 15000 });
vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});
const apiMock = vi.mocked(api);

const MARKDOWN =
  "# Deploying\n\n## Night deploys\n<!-- r:7f3a9c2e -->\n\nDeploys run **at** night.\nDeploys run at night, after the batch.\n";

function thread(id: string, overrides: Record<string, unknown> = {}) {
  return {
    id,
    path: "guides/deploying.md",
    anchor: {
      section: "7f3a9c2e",
      quote: "Deploys run at night",
      occurrence: 1,
      state: "found",
    },
    status: "open",
    waiting_on: "agent",
    opened_at: "2026-09-30T10:00:00Z",
    age_secs: 3600,
    entries: [
      {
        at: "2026-09-30T10:00:00Z",
        kind: "comment",
        author: { name: "vera", agent: false },
        text: "Still true?",
      },
    ],
    ...overrides,
  };
}

function serve(
  threads: unknown[],
  routes: Record<string, Answer> = {},
  markdown = MARKDOWN,
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "vera", role: "viewer" }) }),
      "/domains": () => ({ behavior: [], domains: [] }),
      "/reflection/domains": () => ({ domains: [] }),
      "/domains/eng/reflection": () => ({
        domain: "eng",
        enabled: true,
        state: "on",
        home: "local",
        home_url: null,
        right: "read",
        engrams_visible: true,
        member: false,
        pending_engrams: [],
      }),
      "/domains/eng/reflection/tree": () => ({
        domain: "eng",
        pact: { path: "PACT.md", title: "PACT", exists: true, open_threads: 0 },
        nodes: [],
      }),
      "/domains/eng/reflections/guides/deploying.md": () => ({
        domain: "eng",
        path: "guides/deploying.md",
        title: "Deploying",
        markdown,
        checksum: "c1",
        sections: [
          {
            id: "7f3a9c2e",
            heading: "Night deploys",
            line: 3,
            changed: false,
            sources: [],
          },
        ],
        threads,
      }),
      ...routes,
    }),
  );
}

/** The page's answer, with the threads `threads()` gives at the time. */
function docWith(threads: () => unknown[]): Answer {
  return () => ({
    domain: "eng",
    path: "guides/deploying.md",
    title: "Deploying",
    markdown: MARKDOWN,
    checksum: "c1",
    sections: [
      {
        id: "7f3a9c2e",
        heading: "Night deploys",
        line: 3,
        changed: false,
        sources: [],
      },
    ],
    threads: threads(),
  });
}

/** The page with one changed source on its section, and the threads given. */
function docWithSources(threads: unknown[] = [thread("t-00000001")]): Answer {
  return () => ({
    domain: "eng",
    path: "guides/deploying.md",
    title: "Deploying",
    markdown: MARKDOWN,
    checksum: "c1",
    sections: [
      {
        id: "7f3a9c2e",
        heading: "Night deploys",
        line: 3,
        changed: false,
        sources: [
          { permalink: "runbooks/deploy", title: "Deploy", state: "changed" },
        ],
      },
    ],
    threads,
  });
}

/** A request's JSON body. */
function bodyOf(init: RequestInit | undefined): unknown {
  return JSON.parse(typeof init?.body === "string" ? init.body : "null");
}

/** A fake highlight registry, so the paint is observable in jsdom. */
const registry = new Map<string, { ranges: Range[] }>();
class FakeHighlight {
  ranges: Range[];
  constructor(...ranges: Range[]) {
    this.ranges = ranges;
  }
}
const host = globalThis as unknown as { CSS?: unknown; Highlight?: unknown };
const saved = { CSS: host.CSS, Highlight: host.Highlight };

beforeEach(() => {
  apiMock.mockReset();
  registry.clear();
  host.CSS = { highlights: registry };
  host.Highlight = FakeHighlight;
});
afterEach(() => {
  vi.restoreAllMocks();
  host.CSS = saved.CSS;
  host.Highlight = saved.Highlight;
  window.getSelection()?.removeAllRanges();
});

/** Select from the start of one rendered text to an offset in another, as a reader would. */
function select(from: Text, to: Text, toOffset: number): void {
  const range = document.createRange();
  range.setStart(from, 0);
  range.setEnd(to, toOffset);
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
  act(() => {
    document.dispatchEvent(new Event("selectionchange"));
  });
}

/** The page's own text node holding `words`, and where they start in it. */
function pageText(words: string): { node: Text; at: number } {
  const root = document.querySelector("[data-section]")?.closest("article");
  if (!root) {
    throw new Error("no page");
  }
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const at = (node.textContent ?? "").indexOf(words);
    if (at >= 0) {
      return { node: node as Text, at };
    }
  }
  throw new Error(`no page text holding "${words}"`);
}

/** The first rendered text node that starts with `prefix`. */
function textNode(prefix: string): Text {
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (
      (node.textContent ?? "").startsWith(prefix) &&
      !onPaper(node.parentElement)
    ) {
      return node as Text;
    }
  }
  throw new Error(`no text starting with "${prefix}"`);
}

/** Whether an element is in a print-only block, which the screen never shows. */
function onPaper(element: Element | null): boolean {
  return element?.closest("[data-print]") != null;
}

/** The one element showing `text` on screen, the printed Notes left out. */
async function findOnScreen(text: string): Promise<HTMLElement> {
  const found = (await screen.findAllByText(text)).filter(
    (element) => !onPaper(element),
  );
  expect(found).toHaveLength(1);
  return found[0] as HTMLElement;
}

describe("comments on a reflection", () => {
  it("quotes what a key left selected even when Tab collapsed it before the press", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode("Deploys run "), 4);
    const unheard = (event: Event) => {
      event.stopImmediatePropagation();
    };
    window.addEventListener("selectionchange", unheard, true);
    try {
      window.getSelection()?.getRangeAt(0).setEnd(textNode("Deploys run "), 7);
      await new Promise((resolve) => setTimeout(resolve, 0));
      // The key comes up: the page reads the selection then.
      act(() => {
        document.dispatchEvent(
          new KeyboardEvent("keyup", { key: "ArrowRight" }),
        );
      });
      // Tab moves on and the selection collapses outside the page.
      window.getSelection()?.collapse(document.body, 0);
      await new Promise((resolve) => setTimeout(resolve, 0));
    } finally {
      window.removeEventListener("selectionchange", unheard, true);
    }
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    expect(screen.getByText("Deploys")).toBeVisible();
    expect(screen.queryByText("Depl")).toBeNull();
  });

  it("quotes the selection as it stands when Comment is pressed", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode("Deploys run "), 4);
    // The browser's last step of a keyboard selection, with no
    // selectionchange the page has read yet.
    const unheard = (event: Event) => {
      event.stopImmediatePropagation();
    };
    window.addEventListener("selectionchange", unheard, true);
    try {
      window.getSelection()?.getRangeAt(0).setEnd(textNode("Deploys run "), 7);
      await new Promise((resolve) => setTimeout(resolve, 0));
    } finally {
      window.removeEventListener("selectionchange", unheard, true);
    }
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    expect(screen.getByText("Deploys")).toBeVisible();
    expect(screen.queryByText("Depl")).toBeNull();
  });

  it("anchors a comment on the selected passage, source markers included", async () => {
    const opened = vi.fn((_path: string, _init?: RequestInit) =>
      thread("t-00000001"),
    );
    serve([], {
      "/domains/eng/reflection/threads": (path, init) => opened(path, init),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    expect(screen.getByText("Deploys run **at** night")).toBeVisible();
    expect(screen.getByRole("textbox", { name: "Your comment" })).toHaveFocus();
    await userEvent.type(
      screen.getByRole("textbox", { name: "Your comment" }),
      "Still true?",
    );
    await userEvent.click(
      within(screen.getByRole("form", { name: "New comment" })).getByRole(
        "button",
        { name: "Comment" },
      ),
    );
    await waitFor(() => {
      expect(opened).toHaveBeenCalledTimes(1);
    });
    expect(bodyOf(opened.mock.calls[0]?.[1])).toEqual({
      path: "guides/deploying.md",
      section: "7f3a9c2e",
      quote: "Deploys run **at** night",
      occurrence: 1,
      text: "Still true?",
    });
    // The selection went with the send, and the button with it: focus
    // waits on the column's heading.
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "Threads" })).toHaveFocus();
    });
    expect(screen.queryByRole("button", { name: "Comment" })).toBeNull();
  });

  it("lets go of the page passage when text in a thread is selected", async () => {
    serve([thread("t-00000001")]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await findOnScreen("Still true?");
    select(textNode("Deploys run "), textNode(" night."), 6);
    expect(screen.getByRole("button", { name: "Comment" })).toBeVisible();
    const words = textNode("Still true?");
    select(words, words, 5);
    expect(screen.queryByRole("button", { name: "Comment" })).toBeNull();
    expect(
      screen.getByText("Select text in the page to comment on it."),
    ).toBeVisible();
  });

  it("returns focus to the Comment button when the comment is cancelled", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("form", { name: "New comment" })).toBeNull();
    expect(screen.getByRole("button", { name: "Comment" })).toHaveFocus();
  });

  it("puts a passage it cannot pin on the section", async () => {
    const opened = vi.fn((_path: string, _init?: RequestInit) =>
      thread("t-00000001"),
    );
    serve(
      [],
      {
        "/domains/eng/reflection/threads": (path, init) => opened(path, init),
      },
      "# Deploying\n\n## Night deploys\n<!-- r:7f3a9c2e -->\n\nTom &amp; Jerry deploy.\n",
    );
    renderApp("/d/eng/reflection/guides/deploying.md");
    const words = await screen.findByText(/Jerry deploy/);
    const text = words.firstChild as Text;
    select(text, text, 3);
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    expect(
      screen.getByText(
        "This passage cannot be pinned exactly, so the comment goes on the section.",
      ),
    ).toBeVisible();
    await userEvent.type(
      screen.getByRole("textbox", { name: "Your comment" }),
      "Who?",
    );
    await userEvent.click(
      within(screen.getByRole("form", { name: "New comment" })).getByRole(
        "button",
        { name: "Comment" },
      ),
    );
    await waitFor(() => {
      expect(opened).toHaveBeenCalledTimes(1);
    });
    expect(bodyOf(opened.mock.calls[0]?.[1])).toEqual({
      path: "guides/deploying.md",
      section: "7f3a9c2e",
      text: "Who?",
    });
  });

  it("shows the anonymous visitor every thread and no way to write one", async () => {
    serve([thread("t-00000001")], {
      "/auth/me": () => meResponse({ user: null, anonymous: true }),
      "/domains/eng/reflection/threads": () => ({
        threads: [
          thread("t-00000002", {
            status: "resolved",
            entries: [
              {
                at: "2026-09-30T11:00:00Z",
                kind: "resolve",
                author: { name: "ann", agent: false },
                text: "Done.",
              },
            ],
          }),
        ],
      }),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    expect(
      await screen.findByRole("article", { name: "Thread on Night deploys" }),
    ).toBeVisible();
    await userEvent.click(
      screen.getByRole("switch", { name: "Show resolved threads" }),
    );
    expect(await screen.findByText("Resolved: Done.")).toBeVisible();
    for (const name of ["Comment", "Answer", "Resolve", "Reopen"]) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }
  });

  it("offers no way to write on a server that serves read-only", async () => {
    serve([thread("t-00000001")], {
      "/auth/me": () =>
        meResponse({
          user: userFixture({ name: "vera", role: "viewer" }),
          read_only: true,
        }),
      "/domains/eng/reflection/threads": () => ({
        threads: [
          thread("t-00000002", {
            status: "resolved",
            entries: [
              {
                at: "2026-09-30T11:00:00Z",
                kind: "resolve",
                author: { name: "ann", agent: false },
                text: "Done.",
              },
            ],
          }),
        ],
      }),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    expect(
      await screen.findByRole("article", { name: "Thread on Night deploys" }),
    ).toBeVisible();
    await userEvent.click(
      screen.getByRole("switch", { name: "Show resolved threads" }),
    );
    expect(await screen.findByText("Resolved: Done.")).toBeVisible();
    for (const name of ["Comment", "Answer", "Resolve", "Reopen"]) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }
  });

  it("shows a hint until something is selected, then the Comment button in its place", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const hint = await screen.findByText(
      "Select text in the page to comment on it.",
    );
    expect(hint).toBeVisible();
    expect(screen.queryByRole("button", { name: "Comment" })).toBeNull();
    select(textNode("Deploys run"), textNode("Deploys run"), 6);
    const button = screen.getByRole("button", { name: "Comment" });
    expect(button).toBeVisible();
    expect(
      screen.queryByText("Select text in the page to comment on it."),
    ).toBeNull();
    // A collapse on the page takes the button back to the hint.
    const caret = textNode("Deploys run");
    select(caret, caret, 0);
    expect(screen.queryByRole("button", { name: "Comment" })).toBeNull();
    expect(
      screen.getByText("Select text in the page to comment on it."),
    ).toBeVisible();
  });

  it("a thread whose passage was rewritten shows the old quote and says the text changed", async () => {
    // The server answered "found" before an agent rewrote the passage; the
    // text on screen no longer holds it, and that is what counts.
    serve([
      thread("t-00000001", {
        anchor: {
          section: "7f3a9c2e",
          quote: "Deploys run in the morning",
          occurrence: 1,
          state: "found",
        },
      }),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    expect(within(card).getByText("Deploys run in the morning")).toBeVisible();
    expect(within(card).getByText("The text changed.")).toBeVisible();
    await waitFor(() => {
      expect(registry.get(QUOTE_HIGHLIGHT)?.ranges ?? []).toHaveLength(0);
    });
  });

  it("two threads on the same passage are both listed and both painted", async () => {
    serve([
      thread("t-00000001"),
      thread("t-00000002", {
        entries: [
          {
            at: "2026-09-30T10:01:00Z",
            kind: "comment",
            author: { name: "ann", agent: false },
            text: "Same question.",
          },
        ],
      }),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    expect(
      await screen.findAllByRole("article", {
        name: "Thread on Night deploys",
      }),
    ).toHaveLength(2);
    expect(await findOnScreen("Still true?")).toBeVisible();
    expect(await findOnScreen("Same question.")).toBeVisible();
    await waitFor(() => {
      const ranges = registry.get(QUOTE_HIGHLIGHT)?.ranges ?? [];
      expect(ranges).toHaveLength(2);
      expect(ranges.map((r) => r.toString())).toEqual([
        "Deploys run at night",
        "Deploys run at night",
      ]);
    });
  });

  it("shows a quote cut inside emphasis without the stray markers", async () => {
    serve([
      thread("t-00000001", {
        anchor: {
          section: "7f3a9c2e",
          quote: "at** night",
          occurrence: 1,
          state: "found",
        },
      }),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    expect(within(card).getByText("at night")).toBeVisible();
    expect(within(card).queryByText("The text changed.")).toBeNull();
  });

  it("lists a thread whose section is gone under its own heading", async () => {
    serve([
      thread("t-00000001", {
        anchor: {
          section: "0b1c2d3e",
          quote: "Old text",
          occurrence: 1,
          state: "section_gone",
        },
      }),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    expect(
      await screen.findByRole("article", { name: "Thread on the page" }),
    ).toBeVisible();
    expect(
      screen.getByRole("heading", { name: "Not on the page any more" }),
    ).toBeVisible();
    expect(await findOnScreen("Old text")).toBeVisible();
  });

  it("resolves with a short outcome and reopens visibly", async () => {
    const steps: { url: string; body: unknown }[] = [];
    // The server's state: the page lists the thread while it is open, the
    // resolved list while it is resolved.
    let status: "open" | "resolved" = "open";
    const resolvedThread = thread("t-00000001", {
      status: "resolved",
      entries: [
        {
          at: "2026-09-30T10:00:00Z",
          kind: "comment",
          author: { name: "vera", agent: false },
          text: "Still true?",
        },
        {
          at: "2026-09-30T11:00:00Z",
          kind: "resolve",
          author: { name: "claude-code", agent: true },
          text: "Yes, still at night.",
        },
      ],
    });
    const docRoute = docWith(() =>
      status === "open" ? [thread("t-00000001")] : [],
    );
    serve([], {
      "/domains/eng/reflections/guides/deploying.md": docRoute,
      "/domains/eng/reflection/threads/t-00000001/resolve": (url, init) => {
        steps.push({ url, body: bodyOf(init) });
        status = "resolved";
        return resolvedThread;
      },
      "/domains/eng/reflection/threads": () => ({
        threads: status === "resolved" ? [resolvedThread] : [],
      }),
      "/domains/eng/reflection/threads/t-00000001/reopen": (url, init) => {
        steps.push({ url, body: bodyOf(init) });
        status = "open";
        return thread("t-00000001");
      },
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    await userEvent.click(
      within(card).getByRole("button", { name: "Resolve" }),
    );
    await userEvent.type(
      within(card).getByRole("textbox", { name: "Outcome" }),
      "Yes, still at night.",
    );
    await userEvent.click(
      within(card).getByRole("button", { name: "Resolve" }),
    );
    await waitFor(() => {
      expect(steps[0]).toEqual({
        url: "/domains/eng/reflection/threads/t-00000001/resolve",
        body: { path: "guides/deploying.md", outcome: "Yes, still at night." },
      });
    });
    await waitFor(() => {
      expect(
        screen.queryByRole("article", { name: "Thread on Night deploys" }),
      ).toBeNull();
    });
    expect(screen.getByRole("heading", { name: "Threads" })).toHaveFocus();
    await userEvent.click(
      screen.getByRole("switch", { name: "Show resolved threads" }),
    );
    const resolved = await screen.findByText("Resolved: Yes, still at night.");
    const resolvedCard = resolved.closest("article") as HTMLElement;
    expect(resolvedCard).toHaveAccessibleName("Thread on Night deploys");
    expect(within(resolvedCard).getByText("(agent)")).toBeVisible();
    await userEvent.click(
      within(resolvedCard).getByRole("button", { name: "Reopen" }),
    );
    await waitFor(() => {
      expect(steps[1]?.url).toBe(
        "/domains/eng/reflection/threads/t-00000001/reopen",
      );
    });
    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: "Resolve" }),
      ).toBeInTheDocument();
    });
    // Reopened: listed once, as an open thread.
    expect(
      screen.getAllByRole("article", { name: "Thread on Night deploys" }),
    ).toHaveLength(1);
    expect(screen.getByRole("heading", { name: "Threads" })).toHaveFocus();
  });

  it("answers with the text and reads the page again", async () => {
    const answered = vi.fn((_path: string, _init?: RequestInit) =>
      thread("t-00000001"),
    );
    const docRoute = vi.fn(docWith(() => [thread("t-00000001")]));
    serve([], {
      "/domains/eng/reflections/guides/deploying.md": docRoute,
      "/domains/eng/reflection/threads/t-00000001/answer": (path, init) =>
        answered(path, init),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    const reads = docRoute.mock.calls.length;
    await userEvent.click(within(card).getByRole("button", { name: "Answer" }));
    await userEvent.type(
      within(card).getByRole("textbox", { name: "Your answer" }),
      "Yes.",
    );
    await userEvent.click(within(card).getByRole("button", { name: "Answer" }));
    await waitFor(() => {
      expect(answered).toHaveBeenCalledTimes(1);
    });
    expect(bodyOf(answered.mock.calls[0]?.[1])).toEqual({
      path: "guides/deploying.md",
      text: "Yes.",
    });
    await waitFor(() => {
      expect(docRoute.mock.calls.length).toBeGreaterThan(reads);
    });
    // The form closes once the answer landed, and focus goes back to the
    // button that opened it.
    await waitFor(() => {
      expect(
        within(card).queryByRole("textbox", { name: "Your answer" }),
      ).toBeNull();
    });
    expect(within(card).getByRole("button", { name: "Answer" })).toHaveFocus();
  });

  it("shows an open thread with two quiet buttons and no form", async () => {
    serve([thread("t-00000001")]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    expect(within(card).getByRole("button", { name: "Answer" })).toBeVisible();
    expect(within(card).getByRole("button", { name: "Resolve" })).toBeVisible();
    expect(within(card).queryByRole("textbox")).toBeNull();
    expect(within(card).queryByRole("button", { name: "Cancel" })).toBeNull();
  });

  it("opens only the pressed form, with focus in its field", async () => {
    serve([thread("t-00000001")]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    await userEvent.click(
      within(card).getByRole("button", { name: "Resolve" }),
    );
    expect(
      within(card).getByRole("textbox", { name: "Outcome" }),
    ).toHaveFocus();
    expect(
      within(card).queryByRole("textbox", { name: "Your answer" }),
    ).toBeNull();
    expect(within(card).getAllByRole("textbox")).toHaveLength(1);
    expect(within(card).getByRole("button", { name: "Resolve" })).toBeVisible();
    expect(within(card).getByRole("button", { name: "Cancel" })).toBeVisible();
  });

  it("closes the form on Cancel and returns focus to the button that opened it", async () => {
    serve([thread("t-00000001")]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    await userEvent.click(within(card).getByRole("button", { name: "Answer" }));
    await userEvent.click(within(card).getByRole("button", { name: "Cancel" }));
    expect(within(card).queryByRole("textbox")).toBeNull();
    expect(within(card).getByRole("button", { name: "Answer" })).toHaveFocus();
    await userEvent.click(
      within(card).getByRole("button", { name: "Resolve" }),
    );
    await userEvent.click(within(card).getByRole("button", { name: "Cancel" }));
    expect(within(card).getByRole("button", { name: "Resolve" })).toHaveFocus();
  });

  it("opens one form at a time in a card and leaves other cards alone", async () => {
    serve([
      thread("t-00000001"),
      thread("t-00000002", {
        anchor: {
          section: "7f3a9c2e",
          quote: null,
          occurrence: 1,
          state: "found",
        },
      }),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const cards = await screen.findAllByRole("article", {
      name: "Thread on Night deploys",
    });
    expect(cards).toHaveLength(2);
    await userEvent.click(
      within(cards[0] as HTMLElement).getByRole("button", { name: "Answer" }),
    );
    // Within a card the opened form replaces the row, so Resolve is not
    // offered beside an open Answer form.
    expect(
      within(cards[0] as HTMLElement).queryByRole("button", {
        name: "Resolve",
      }),
    ).toBeNull();
    expect(
      within(cards[1] as HTMLElement).getByRole("button", { name: "Resolve" }),
    ).toBeVisible();
  });

  it("keeps the form open with the server's text when a send fails", async () => {
    serve([thread("t-00000001")], {
      "/domains/eng/reflection/threads/t-00000001/resolve": () => {
        throw new ApiProblem(
          409,
          "Conflict",
          "Someone else resolved this thread.",
        );
      },
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    await userEvent.click(
      within(card).getByRole("button", { name: "Resolve" }),
    );
    await userEvent.type(
      within(card).getByRole("textbox", { name: "Outcome" }),
      "Done.",
    );
    await userEvent.click(
      within(card).getByRole("button", { name: "Resolve" }),
    );
    expect(await within(card).findByRole("alert")).toHaveTextContent(
      "Someone else resolved this thread.",
    );
    expect(within(card).getByRole("textbox", { name: "Outcome" })).toHaveValue(
      "Done.",
    );
  });

  it("puts the Comment button under the Threads header and keeps the selection on press", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const heading = await screen.findByRole("heading", { name: "Threads" });
    await screen.findByText("Deploys run");
    select(textNode("Deploys run"), textNode("Deploys run"), 6);
    const button = screen.getByRole("button", { name: "Comment" });
    expect(button.previousElementSibling).toBe(heading.parentElement);
    const down = new MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
    });
    button.dispatchEvent(down);
    expect(down.defaultPrevented).toBe(true);
    expect(window.getSelection()?.toString()).not.toBe("");
  });

  it("paints the thread under the pointer a second time, and clears it all on leaving", async () => {
    serve([thread("t-00000001")]);
    const view = renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    await waitFor(() => {
      expect(registry.get(QUOTE_HIGHLIGHT)?.ranges ?? []).toHaveLength(1);
    });
    expect(registry.get(ACTIVE_HIGHLIGHT)?.ranges ?? []).toHaveLength(0);
    fireEvent.mouseEnter(card.parentElement as HTMLElement);
    await waitFor(() => {
      expect(
        (registry.get(ACTIVE_HIGHLIGHT)?.ranges ?? []).map((r) => r.toString()),
      ).toEqual(["Deploys run at night"]);
    });
    fireEvent.mouseLeave(card.parentElement as HTMLElement);
    await waitFor(() => {
      expect(registry.get(ACTIVE_HIGHLIGHT)?.ranges ?? []).toHaveLength(0);
    });
    view.unmount();
    expect(registry.size).toBe(0);
  });

  it("gives a thread its own id as the address, and a link to it scrolls there", async () => {
    const scrolled = vi.spyOn(Element.prototype, "scrollIntoView");
    serve([thread("t-00000001")]);
    renderApp("/d/eng/reflection/guides/deploying.md#t-00000001");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    expect(card).toHaveAttribute("id", "t-00000001");
    await waitFor(() => {
      expect(scrolled.mock.contexts).toContain(card);
    });
  });

  it("says a reopen with no words was reopened", async () => {
    serve([
      thread("t-00000001", {
        entries: [
          {
            at: "2026-09-30T10:00:00Z",
            kind: "reopen",
            author: { name: "vera", agent: false },
            text: "",
          },
        ],
      }),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    expect(within(card).getByText("Reopened")).toBeVisible();
  });

  it("shows a quote it cannot find on the page as it was stored", async () => {
    serve([
      thread("t-00000001", {
        anchor: {
          section: "7f3a9c2e",
          quote: "at** dawn",
          occurrence: 1,
          state: "text_changed",
        },
      }),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    expect(within(card).getByText("at** dawn")).toBeVisible();
  });

  it("treats an empty quote as a comment on the section", async () => {
    serve([
      thread("t-00000001", {
        anchor: {
          section: "7f3a9c2e",
          quote: "",
          occurrence: 1,
          state: "found",
        },
      }),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    expect(card.querySelector("blockquote")).toBeNull();
    expect(within(card).queryByText("The text changed.")).toBeNull();
  });

  it("lists resolved threads per section, a gone one under its own heading", async () => {
    serve([], {
      "/domains/eng/reflection/threads": () => ({
        threads: [
          thread("t-00000003", {
            status: "resolved",
            anchor: {
              section: "0b1c2d3e",
              quote: "Old text",
              occurrence: 1,
              state: "section_gone",
            },
          }),
        ],
      }),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    await userEvent.click(
      await screen.findByRole("switch", { name: "Show resolved threads" }),
    );
    expect(
      await screen.findByRole("heading", { name: "Not on the page any more" }),
    ).toBeVisible();
    expect(
      screen.getByRole("article", { name: "Thread on the page" }),
    ).toBeVisible();
  });

  it("says when the resolved threads cannot be read", async () => {
    serve([], {
      "/domains/eng/reflection/threads": () => {
        throw new ApiProblem(500, "Internal Server Error", "the index is busy");
      },
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    await userEvent.click(
      await screen.findByRole("switch", { name: "Show resolved threads" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "the index is busy",
    );
  });

  it("shows resolved threads behind a switch, named for screen readers in full", async () => {
    serve([thread("t-00000001")], {
      "/domains/eng/reflection/threads": () => ({
        threads: [
          thread("t-00000002", {
            status: "resolved",
            entries: [
              {
                at: "2026-09-30T10:00:00Z",
                kind: "resolve",
                author: { name: "vera", agent: false },
                text: "Done.",
              },
            ],
          }),
        ],
      }),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    const toggle = await screen.findByRole("switch", {
      name: "Show resolved threads",
    });
    expect(toggle).toHaveTextContent("Show resolved");
    expect(toggle).toHaveAttribute("aria-checked", "false");
    expect(screen.getByText("1 open")).toBeVisible();
    expect(screen.queryByText("Resolved: Done.")).toBeNull();
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "true");
    expect(await screen.findByText("Resolved: Done.")).toBeVisible();
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "false");
    expect(screen.queryByText("Resolved: Done.")).toBeNull();
  });

  it("says a person's thread was answered and waits for them, without Show resolved", async () => {
    const answered = (who: string) => [
      {
        at: "2026-09-30T10:00:00Z",
        kind: "comment",
        author: { name: who, agent: false },
        text: "Still true?",
      },
      {
        at: "2026-09-30T11:00:00Z",
        kind: "comment",
        author: { name: "scribe", agent: true },
        text: "Yes, still true.",
      },
    ];
    serve([
      thread("t-00000001", { waiting_on: "people", entries: answered("vera") }),
      thread("t-00000002", { waiting_on: "people", entries: answered("ann") }),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const [mine, theirs] = (await screen.findAllByRole("article", {
      name: "Thread on Night deploys",
    })) as [HTMLElement, HTMLElement];
    expect(
      screen.getByRole("switch", { name: "Show resolved threads" }),
    ).toHaveAttribute("aria-checked", "false");
    expect(within(mine).getByText("Answered, waiting for you")).toBeVisible();
    expect(within(theirs).getByText("Answered, waiting for ann")).toBeVisible();
    expect(within(mine).queryByText("Waiting for people")).toBeNull();
  });

  it("words who a thread waits on", () => {
    const view = (entries: unknown[], waitingOn: "people" | "agent") =>
      ({
        id: "t-1",
        path: "a.md",
        anchor: { section: "s", quote: null, occurrence: 1, state: "found" },
        status: "open",
        waitingOn,
        entries,
      }) as unknown as ThreadView;
    const person = (name: string, kind = "comment") => ({
      at: "",
      kind,
      author: { name, agent: false },
      text: "x",
    });
    const agent = {
      at: "",
      kind: "comment",
      author: { name: "a", agent: true },
      text: "y",
    };
    expect(waitingWords(view([person("vera")], "agent"), "vera").words).toBe(
      "Waiting for the agent",
    );
    // An agent's own thread waits for people in general.
    expect(waitingWords(view([agent], "people"), "vera").words).toBe(
      "Waiting for people",
    );
    // The stored name is the sanitized account name.
    expect(
      waitingWords(view([person("Ada ops"), agent], "people"), "Ada (ops)")
        .words,
    ).toBe("Answered, waiting for you");
    expect(
      waitingWords(view([person("vera"), agent], "people"), null).words,
    ).toBe("Answered, waiting for vera");
    // The same white space as the core: U+0085 splits, a byte order mark
    // does not.
    expect(
      waitingWords(
        view([person("Ada Lovelace"), agent], "people"),
        "Ada\u0085Lovelace",
      ).words,
    ).toBe("Answered, waiting for you");
    expect(
      waitingWords(view([person("\ufeff"), agent], "people"), "\ufeff").words,
    ).toBe("Answered, waiting for you");
  });

  it("draws a thread as quote, avatar, name and time, quiet actions and who it waits on", async () => {
    serve([
      thread("t-00000001"),
      thread("t-00000002", {
        waiting_on: "people",
        anchor: {
          section: "7f3a9c2e",
          quote: null,
          occurrence: 1,
          state: "found",
        },
        entries: [
          {
            at: "2026-09-30T10:00:00Z",
            kind: "comment",
            author: { name: "scribe", agent: true },
            text: "Checked.",
          },
        ],
      }),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const [first, second] = (await screen.findAllByRole("article", {
      name: "Thread on Night deploys",
    })) as [HTMLElement, HTMLElement];
    expect(first.querySelector("blockquote")).toHaveTextContent(
      "Deploys run at night",
    );
    const avatar = first.querySelector("[data-avatar]");
    expect(avatar).toHaveTextContent("V");
    expect(avatar).toHaveAttribute("aria-hidden", "true");
    expect(within(first).getByText("vera")).toHaveClass("font-semibold");
    expect(within(first).getByText("Still true?")).toBeVisible();
    // Quiet text actions, not boxed buttons.
    for (const name of ["Answer", "Resolve"]) {
      const button = within(first).getByRole("button", { name });
      expect(button).toHaveClass("text-link");
      expect(button).not.toHaveClass("border");
    }
    expect(within(first).getByText("Waiting for the agent")).toBeVisible();
    expect(within(second).getByText("Waiting for people")).toBeVisible();
    expect(second.querySelector("[data-avatar]")).toHaveTextContent("S");
    expect(within(second).getByText("(agent)")).toBeVisible();
    // Threads are parted by hairlines, not drawn as boxes.
    expect(first).not.toHaveClass("border");
    expect(first.parentElement).toHaveClass("border-t");
  });

  it("gives the picked thread the wash and paints its quote as the active one", async () => {
    serve([
      thread("t-00000001"),
      thread("t-00000002", {
        anchor: {
          section: "7f3a9c2e",
          quote: "after the batch",
          occurrence: 1,
          state: "found",
        },
      }),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const [first, second] = (await screen.findAllByRole("article", {
      name: "Thread on Night deploys",
    })) as [HTMLElement, HTMLElement];
    await waitFor(() => {
      expect(registry.get(QUOTE_HIGHLIGHT)?.ranges ?? []).toHaveLength(2);
    });
    expect(first.parentElement).not.toHaveAttribute("data-picked");
    await userEvent.click(within(second).getByText("Still true?"));
    expect(second.parentElement).toHaveAttribute("data-picked");
    expect(second.parentElement).toHaveClass("bg-accent-50");
    // Muted text reads as ink-3 on the wash, to keep 4.5:1.
    expect(second.parentElement).toHaveClass("[&_.text-muted]:text-ink-3");
    expect(second.querySelector("blockquote")).toHaveClass("border-accent-400");
    expect(first.parentElement).not.toHaveAttribute("data-picked");
    await waitFor(() => {
      expect(
        (registry.get(ACTIVE_HIGHLIGHT)?.ranges ?? []).map((r) => r.toString()),
      ).toEqual(["after the batch"]);
    });
    // The pick stays when the pointer leaves the column.
    fireEvent.mouseLeave(second.parentElement as HTMLElement);
    expect(second.parentElement).toHaveAttribute("data-picked");
  });

  it("picks a thread from a tap on its quote in the text", async () => {
    serve([thread("t-00000001")]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    await waitFor(() => {
      expect(registry.get(QUOTE_HIGHLIGHT)?.ranges ?? []).toHaveLength(1);
    });
    const { node: words, at } = pageText("Deploys run at night, after");
    const spot = vi.fn(() => {
      const range = document.createRange();
      range.setStart(words, at + 3);
      range.collapse(true);
      return range;
    });
    const doc = document as unknown as { caretRangeFromPoint?: unknown };
    doc.caretRangeFromPoint = spot;
    try {
      fireEvent.click(words.parentElement as HTMLElement);
      expect(card.parentElement).toHaveAttribute("data-picked");
      await waitFor(() => {
        expect(registry.get(ACTIVE_HIGHLIGHT)?.ranges ?? []).toHaveLength(1);
      });
      // A tap on text with no quote lets the pick go.
      spot.mockImplementation(() => {
        const range = document.createRange();
        range.setStart(words, words.length - 1);
        range.collapse(true);
        return range;
      });
      fireEvent.click(words.parentElement as HTMLElement);
      expect(card.parentElement).not.toHaveAttribute("data-picked");
    } finally {
      delete doc.caretRangeFromPoint;
    }
  });

  it("anchors the same with a sources count at the heading, which is no part of the text", async () => {
    const opened = vi.fn((_path: string, _init?: RequestInit) =>
      thread("t-00000001"),
    );
    serve(
      [
        thread("t-00000009", {
          anchor: {
            section: "7f3a9c2e",
            quote: "Night deploys",
            occurrence: 1,
            state: "found",
          },
        }),
      ],
      {
        "/domains/eng/reflection/threads": (path, init) => opened(path, init),
        "/domains/eng/reflections/guides/deploying.md": () => ({
          domain: "eng",
          path: "guides/deploying.md",
          title: "Deploying",
          markdown: MARKDOWN,
          checksum: "c1",
          sections: [
            {
              id: "7f3a9c2e",
              heading: "Night deploys",
              line: 3,
              changed: false,
              sources: [
                {
                  permalink: "runbooks/deploy",
                  title: "Deploy",
                  state: "changed",
                },
              ],
            },
          ],
          threads: [
            thread("t-00000009", {
              anchor: {
                section: "7f3a9c2e",
                quote: "Night deploys",
                occurrence: 1,
                state: "found",
              },
            }),
          ],
        }),
      },
    );
    renderApp("/d/eng/reflection/guides/deploying.md");
    const control = await screen.findByRole("button", {
      name: "1 source for Night deploys",
    });
    expect(control).toHaveTextContent("1 source");
    // The heading keeps its own name, and its quote paints its own words.
    expect(
      screen.getByRole("heading", { name: "Night deploys", level: 2 }),
    ).toBeVisible();
    await waitFor(() => {
      expect(
        (registry.get(QUOTE_HIGHLIGHT)?.ranges ?? []).map((r) => r.toString()),
      ).toEqual(["Night deploys"]);
    });
    // A selection from the heading's words over the control reads as the
    // heading alone.
    const label = textNode("1 source");
    select(textNode("Night deploys"), label, label.length);
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    await userEvent.type(
      screen.getByRole("textbox", { name: "Your comment" }),
      "Which night?",
    );
    await userEvent.click(
      within(screen.getByRole("form", { name: "New comment" })).getByRole(
        "button",
        { name: "Comment" },
      ),
    );
    await waitFor(() => {
      expect(opened).toHaveBeenCalledTimes(1);
    });
    expect(bodyOf(opened.mock.calls[0]?.[1])).toMatchObject({
      section: "7f3a9c2e",
      quote: "Night deploys",
      occurrence: 1,
    });
    // And the body still anchors at its own offsets.
    select(textNode("Deploys run "), textNode(" night."), 6);
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    expect(screen.getByText("Deploys run **at** night")).toBeVisible();
  });

  it("keeps a half-typed comment on its own page", async () => {
    const opened = vi.fn((_path: string, _init?: RequestInit) =>
      thread("t-00000001"),
    );
    serve([], {
      "/domains/eng/reflection/threads": (path, init) => opened(path, init),
      "/domains/eng/reflection/tree": () => ({
        domain: "eng",
        pact: { path: "PACT.md", title: "PACT", exists: true, open_threads: 0 },
        nodes: [
          {
            kind: "reflection",
            name: "deploying.md",
            path: "guides/deploying.md",
            title: "Deploying",
          },
          {
            kind: "reflection",
            name: "other.md",
            path: "other.md",
            title: "Other page",
          },
        ],
      }),
      "/domains/eng/reflections/other.md": () => ({
        domain: "eng",
        path: "other.md",
        title: "Other page",
        markdown:
          "# Other page\n\n## Rollback\n<!-- r:1a2b3c4d -->\n\nOther words.\n",
        checksum: "c2",
        sections: [
          {
            id: "1a2b3c4d",
            heading: "Rollback",
            line: 3,
            changed: false,
            sources: [],
          },
        ],
        threads: [],
      }),
    });
    renderApp("/d/eng/reflection/other.md");
    expect(await screen.findByText("Other words.")).toBeVisible();
    await userEvent.click(
      (
        await screen.findAllByRole("link", { name: "Deploying" })
      )[0] as HTMLElement,
    );
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    await userEvent.type(
      screen.getByRole("textbox", { name: "Your comment" }),
      "Half a th",
    );
    await userEvent.click(
      screen.getAllByRole("link", { name: "Other page" })[0] as HTMLElement,
    );
    expect(await screen.findByText("Other words.")).toBeVisible();
    expect(screen.queryByRole("form", { name: "New comment" })).toBeNull();
    expect(
      screen.getByText("Select text in the page to comment on it."),
    ).toBeVisible();
    // And back on the first page the words are gone too.
    await userEvent.click(
      screen.getAllByRole("link", { name: "Deploying" })[0] as HTMLElement,
    );
    await screen.findByText("at");
    expect(screen.queryByRole("form", { name: "New comment" })).toBeNull();
    expect(opened).not.toHaveBeenCalled();
  });

  it("paints a quote over a heading on the words only, never the sources count", async () => {
    let posted: unknown = null;
    const doc = () => ({
      domain: "eng",
      path: "guides/deploying.md",
      title: "Deploying",
      markdown: MARKDOWN,
      checksum: "c1",
      sections: [
        {
          id: "7f3a9c2e",
          heading: "Night deploys",
          line: 3,
          changed: false,
          sources: [
            { permalink: "runbooks/deploy", title: "Deploy", state: "changed" },
          ],
        },
      ],
      threads:
        posted === null
          ? []
          : [
              thread("t-00000005", {
                anchor: {
                  section: "7f3a9c2e",
                  quote: (posted as { quote: string }).quote,
                  occurrence: 1,
                  state: "found",
                },
              }),
            ],
    });
    serve([], {
      "/domains/eng/reflection/threads": (_path, init) => {
        posted = bodyOf(init);
        return thread("t-00000005");
      },
      "/domains/eng/reflections/guides/deploying.md": doc,
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByRole("button", { name: "1 source for Night deploys" });
    select(textNode("Night deploys"), textNode(" night."), 6);
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    await userEvent.type(
      screen.getByRole("textbox", { name: "Your comment" }),
      "Both?",
    );
    await userEvent.click(
      within(screen.getByRole("form", { name: "New comment" })).getByRole(
        "button",
        { name: "Comment" },
      ),
    );
    expect((posted as { quote?: string } | null)?.quote).toContain(
      "Night deploys",
    );
    // The id line under the heading is no part of the section's text, here
    // as in the engine, so a selection over it never quotes it.
    expect((posted as { quote?: string } | null)?.quote).not.toContain("<!--");
    await waitFor(() => {
      expect(registry.get(QUOTE_HIGHLIGHT)?.ranges.length ?? 0).toBeGreaterThan(
        0,
      );
    });
    const painted = (registry.get(QUOTE_HIGHLIGHT)?.ranges ?? [])
      .map((r) => r.toString())
      .join("|");
    expect(painted).toContain("Night deploys");
    expect(painted).toContain("Deploys run");
    expect(painted).not.toContain("source");
  });

  it("leaves the pick alone on a press of the sources count", async () => {
    serve([thread("t-00000001")], {
      "/domains/eng/reflections/guides/deploying.md": docWithSources(),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    await userEvent.click(within(card).getByText("Still true?"));
    expect(card.parentElement).toHaveAttribute("data-picked");
    await userEvent.click(
      screen.getByRole("button", { name: "1 source for Night deploys" }),
    );
    expect(card.parentElement).toHaveAttribute("data-picked");
  });

  it("picks the thread a link names, beside the page too", async () => {
    serve([thread("t-00000001")]);
    renderApp("/d/eng/reflection/guides/deploying.md#t-00000001");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    expect(card.parentElement).toHaveAttribute("data-picked");
    await waitFor(() => {
      expect(
        (registry.get(ACTIVE_HIGHLIGHT)?.ranges ?? []).map((r) => r.toString()),
      ).toEqual(["Deploys run at night"]);
    });
  });

  it("opens a folded callout that holds the quote of the thread a reader picks", async () => {
    serve(
      [
        thread("t-00000001", {
          anchor: {
            section: "7f3a9c2e",
            quote: "after the batch",
            occurrence: 1,
            state: "found",
          },
        }),
      ],
      {},
      "# Deploying\n\n## Night deploys\n<!-- r:7f3a9c2e -->\n\n> [!DETAILS]- Why\n> Deploys run at night, after the batch.\n",
    );
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    await waitFor(() => {
      expect(registry.get(QUOTE_HIGHLIGHT)?.ranges ?? []).toHaveLength(1);
    });
    const fold = document.querySelector("details[data-callout]")!;
    expect(fold).toHaveProperty("open", false);
    await userEvent.click(within(card).getByText("Still true?"));
    await waitFor(() => {
      expect(fold).toHaveProperty("open", true);
    });
  });

  it("says the Comment button is there once per selection, and puts focus on the new thread", async () => {
    let posted = false;
    serve([], {
      "/domains/eng/reflection/threads": () => {
        posted = true;
        return thread("t-00000007");
      },
      "/domains/eng/reflections/guides/deploying.md": docWith(() =>
        posted ? [thread("t-00000007")] : [],
      ),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    const status = screen.getByRole("status", { hidden: true });
    expect(status).toHaveTextContent("");
    select(textNode("Deploys run "), textNode(" night."), 6);
    expect(status).toHaveTextContent("Comment on the selection");
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    await userEvent.type(
      screen.getByRole("textbox", { name: "Your comment" }),
      "Still true?",
    );
    await userEvent.click(
      within(screen.getByRole("form", { name: "New comment" })).getByRole(
        "button",
        { name: "Comment" },
      ),
    );
    await waitFor(() => {
      expect(
        screen.getByRole("article", { name: "Thread on Night deploys" }),
      ).toHaveFocus();
    });
    expect(status).toHaveTextContent("Comment posted.");
  });

  it("quotes over a stray id line in the body with the line, as the engine keeps it", async () => {
    const STRAY =
      "# Deploying\n\n## Night deploys\n<!-- r:7f3a9c2e -->\n\nFirst part.\n\n<!-- r:1a2b3c4d -->\n\nSecond part.\n";
    let posted: { quote?: string } | null = null;
    serve(
      [],
      {
        "/domains/eng/reflection/threads": (_path, init) => {
          posted = bodyOf(init) as { quote?: string };
          return thread("t-00000001");
        },
      },
      STRAY,
    );
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("Second part.");
    expect(document.body.innerHTML).not.toContain("1a2b3c4d");
    select(textNode("First part."), textNode("Second part."), 6);
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    await userEvent.type(
      screen.getByRole("textbox", { name: "Your comment" }),
      "Both?",
    );
    await userEvent.click(
      within(screen.getByRole("form", { name: "New comment" })).getByRole(
        "button",
        { name: "Comment" },
      ),
    );
    await waitFor(() => {
      expect(posted).not.toBeNull();
    });
    // The engine's section text keeps a stray id line (it drops only the
    // one under the heading), so this quote is found there as here.
    expect((posted as { quote?: string } | null)?.quote).toBe(
      "First part.\n\n<!-- r:1a2b3c4d -->\n\nSecond",
    );
  });

  describe("quotes in a fenced code block", () => {
    const CODE =
      "# Deploying\n\n## Night deploys\n<!-- r:7f3a9c2e -->\n\nRun this first:\n\n```sh\ndeployctl rollback --now\n```\n";

    it("paints a quote inside the fence on its own words", async () => {
      serve(
        [
          thread("t-00000001", {
            anchor: {
              section: "7f3a9c2e",
              quote: "deployctl rollback",
              occurrence: 1,
              state: "found",
            },
          }),
        ],
        {},
        CODE,
      );
      renderApp("/d/eng/reflection/guides/deploying.md");
      await screen.findByRole("article", { name: "Thread on Night deploys" });
      await waitFor(() => {
        expect(
          (registry.get(QUOTE_HIGHLIGHT)?.ranges ?? []).map((r) =>
            r.toString(),
          ),
        ).toEqual(["deployctl rollback"]);
      });
    });

    it("paints a quote from a paragraph into the fence all the way", async () => {
      serve(
        [
          thread("t-00000001", {
            anchor: {
              section: "7f3a9c2e",
              quote: "first:\n\n```sh\ndeployctl",
              occurrence: 1,
              state: "found",
            },
          }),
        ],
        {},
        CODE,
      );
      renderApp("/d/eng/reflection/guides/deploying.md");
      await screen.findByRole("article", { name: "Thread on Night deploys" });
      await waitFor(() => {
        const painted = (registry.get(QUOTE_HIGHLIGHT)?.ranges ?? [])
          .map((r) => r.toString())
          .join("");
        expect(painted).toMatch(/^first:/);
        expect(painted).toMatch(/deployctl$/);
      });
    });

    it("quotes a selection inside the fence as its source text", async () => {
      const opened = vi.fn((_path: string, _init?: RequestInit) =>
        thread("t-00000001"),
      );
      serve(
        [],
        {
          "/domains/eng/reflection/threads": (path, init) => opened(path, init),
        },
        CODE,
      );
      renderApp("/d/eng/reflection/guides/deploying.md");
      await screen.findByText("Run this first:");
      const code = document.querySelector("pre code") as HTMLElement;
      await waitFor(() => {
        expect(code.querySelector("[data-src]")).not.toBeNull();
      });
      const first = code.querySelector("[data-src]")?.firstChild as Text;
      const last = Array.from(code.querySelectorAll("[data-src]"))
        .map((span) => span.firstChild as Text)
        .find((text) => text.data.includes("--now")) as Text;
      const range = document.createRange();
      range.setStart(first, 0);
      range.setEnd(last, last.data.indexOf("--now") + "--now".length);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
      act(() => {
        document.dispatchEvent(new Event("selectionchange"));
      });
      await userEvent.click(screen.getByRole("button", { name: "Comment" }));
      await userEvent.type(
        screen.getByRole("textbox", { name: "Your comment" }),
        "Is --now safe?",
      );
      await userEvent.click(
        within(screen.getByRole("form", { name: "New comment" })).getByRole(
          "button",
          { name: "Comment" },
        ),
      );
      await waitFor(() => {
        expect(opened).toHaveBeenCalledTimes(1);
      });
      expect(bodyOf(opened.mock.calls[0]?.[1])).toMatchObject({
        quote: "deployctl rollback --now",
        occurrence: 1,
      });
    });
  });
});

describe("the bar at the selection", () => {
  const bar = () =>
    screen.queryByRole("toolbar", { name: "Selection actions" });
  const barButton = () =>
    screen.getByRole("button", { name: "Comment on selection" });

  it("stands at a selection on the page for a reader who may comment", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    expect(bar()).toBeNull();
    select(textNode("Deploys run "), textNode(" night."), 6);
    const toolbar = bar();
    expect(toolbar).toBeVisible();
    expect(within(toolbar as HTMLElement).getAllByRole("button")).toEqual([
      barButton(),
    ]);
    // It never takes the focus on its own, and Tab passes it by.
    expect(barButton()).not.toHaveFocus();
    expect(barButton()).toHaveAttribute("tabindex", "-1");
    // A collapse on the page takes it away.
    const caret = textNode("Deploys run ");
    select(caret, caret, 0);
    expect(bar()).toBeNull();
  });

  it("is not there for a reader who may not comment", async () => {
    serve([], {
      "/auth/me": () => meResponse({ user: null, anonymous: true }),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    const status = screen.getByRole("status", { hidden: true });
    select(textNode("Deploys run "), textNode(" night."), 6);
    expect(bar()).toBeNull();
    // Nor is the reader told to comment.
    expect(status).toHaveTextContent("");
  });

  it("goes while a comment is written", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    expect(screen.getByRole("form", { name: "New comment" })).toBeVisible();
    expect(bar()).toBeNull();
  });

  it("goes when the selection leaves the page, while the column keeps its button", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    expect(bar()).toBeVisible();
    // A click beside the page leaves a caret outside it.
    window.getSelection()?.collapse(document.body, 0);
    act(() => {
      document.dispatchEvent(new Event("selectionchange"));
    });
    expect(bar()).toBeNull();
    expect(screen.getByRole("button", { name: "Comment" })).toBeVisible();
  });

  it("opens the form on the same passage as the column's button, focus in its field", async () => {
    const opened = vi.fn((_path: string, _init?: RequestInit) =>
      thread("t-00000001"),
    );
    serve([], {
      "/domains/eng/reflection/threads": (path, init) => opened(path, init),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    await userEvent.click(barButton());
    expect(screen.getByText("Deploys run **at** night")).toBeVisible();
    const field = screen.getByRole("textbox", { name: "Your comment" });
    expect(field).toHaveFocus();
    await userEvent.type(field, "Still true?");
    await userEvent.click(
      within(screen.getByRole("form", { name: "New comment" })).getByRole(
        "button",
        { name: "Comment" },
      ),
    );
    await waitFor(() => {
      expect(opened).toHaveBeenCalledTimes(1);
    });
    expect(bodyOf(opened.mock.calls[0]?.[1])).toEqual({
      path: "guides/deploying.md",
      section: "7f3a9c2e",
      quote: "Deploys run **at** night",
      occurrence: 1,
      text: "Still true?",
    });
  });

  it("returns focus to the column's Comment button when the form is cancelled", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    await userEvent.click(barButton());
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("form", { name: "New comment" })).toBeNull();
    expect(screen.getByRole("button", { name: "Comment" })).toHaveFocus();
  });

  it("keeps the selection on a press", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    const down = new MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
    });
    barButton().dispatchEvent(down);
    // jsdom never moves the selection on a press: the cancel is the proof.
    expect(down.defaultPrevented).toBe(true);
  });

  it("goes on Escape until the selection changes", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    expect(bar()).toBeVisible();
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(bar()).toBeNull();
    // The key coming up reads the same selection again: still away.
    fireEvent.keyUp(document.body, { key: "Escape" });
    expect(bar()).toBeNull();
    select(textNode("Deploys run "), textNode("Deploys run "), 7);
    expect(bar()).toBeVisible();
  });

  it("hides while the pointer button is down on the page", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    expect(bar()).toBeVisible();
    fireEvent.pointerDown(textNode("Deploys run ").parentElement as Element, {
      button: 0,
      isPrimary: true,
    });
    expect(bar()).toBeNull();
    fireEvent.pointerUp(document.body, { button: 0, isPrimary: true });
    expect(bar()).toBeVisible();
  });

  it("takes the focus on Alt+F10 and hands it back on Escape, selection kept", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    // The reader was on a control outside the page (a button, not a field).
    const before = screen.getByRole("switch", {
      name: "Show resolved threads",
    });
    before.focus();
    // Without a bar, Alt+F10 does nothing.
    fireEvent.keyDown(before, { key: "F10", altKey: true });
    expect(before).toHaveFocus();
    select(textNode("Deploys run "), textNode(" night."), 6);
    expect(before).toHaveFocus();
    fireEvent.keyDown(before, { key: "F10", altKey: true });
    expect(barButton()).toHaveFocus();
    // A browser may collapse the selection as the bar takes the focus.
    window.getSelection()?.collapse(document.body, 0);
    act(() => {
      document.dispatchEvent(new Event("selectionchange"));
    });
    expect(bar()).toBeVisible();
    fireEvent.keyDown(barButton(), { key: "Escape" });
    expect(bar()).toBeNull();
    expect(before).toHaveFocus();
    expect(window.getSelection()?.toString()).toBe("Deploys run at night");
  });

  it("keeps the passage for a tap whose touch collapsed the selection first", async () => {
    const opened = vi.fn((_path: string, _init?: RequestInit) =>
      thread("t-00000001"),
    );
    serve([], {
      "/domains/eng/reflection/threads": (path, init) => opened(path, init),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    const button = barButton();
    fireEvent.pointerDown(button, { button: 0, isPrimary: true });
    // The touch lands as a caret in the page before the click does.
    const caret = textNode("Deploys run ");
    select(caret, caret, 0);
    fireEvent.pointerUp(button, { button: 0, isPrimary: true });
    fireEvent.click(button);
    const form = screen.getByRole("form", { name: "New comment" });
    expect(within(form).getByText("Deploys run **at** night")).toBeVisible();
  });

  it("says nothing of the bar while a comment is written", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    const status = screen.getByRole("status", { hidden: true });
    select(textNode("Deploys run "), textNode(" night."), 6);
    await userEvent.click(barButton());
    // The first word goes as it always does, after a few seconds.
    await waitFor(
      () => {
        expect(status).toHaveTextContent("");
      },
      { timeout: 6000 },
    );
    // Another passage while the form is open: the bar is not there.
    const caret = textNode("Deploys run ");
    select(caret, caret, 0);
    select(textNode("Deploys run "), textNode("Deploys run "), 7);
    expect(bar()).toBeNull();
    expect(status).toHaveTextContent("");
  });

  describe("where it stands", () => {
    // jsdom lays nothing out: the boxes the bar measures are lent here.
    const sel = { top: 300, bottom: 320, left: 900 };
    const box = (b: {
      top?: number;
      bottom?: number;
      left?: number;
      right?: number;
      width?: number;
    }) =>
      ({
        top: b.top ?? 0,
        bottom: b.bottom ?? 0,
        left: b.left ?? 0,
        right: b.right ?? (b.left ?? 0) + (b.width ?? 0),
        width: b.width ?? (b.right ?? 0) - (b.left ?? 0),
        height: (b.bottom ?? 0) - (b.top ?? 0),
        x: b.left ?? 0,
        y: b.top ?? 0,
        toJSON: () => ({}),
      }) as DOMRect;
    const lent: [string, PropertyDescriptor | undefined][] = [];
    const lend = (name: string, get: (this: HTMLElement) => unknown) => {
      lent.push([
        name,
        Object.getOwnPropertyDescriptor(HTMLElement.prototype, name),
      ]);
      Object.defineProperty(HTMLElement.prototype, name, {
        configurable: true,
        get,
      });
    };
    let observed: (() => void) | null = null;
    const savedObserver = globalThis.ResizeObserver;
    beforeEach(() => {
      Object.assign(sel, { top: 300, bottom: 320, left: 900 });
      vi.spyOn(Range.prototype, "getBoundingClientRect").mockImplementation(
        () => box(sel),
      );
      vi.spyOn(Range.prototype, "getClientRects").mockImplementation(
        () => [box(sel)] as unknown as DOMRectList,
      );
      vi.spyOn(
        HTMLElement.prototype,
        "getBoundingClientRect",
      ).mockImplementation(function (this: HTMLElement) {
        // The shell's frame is the window; the page column ends at 960,
        // where the gap and the threads beside it begin.
        if (this.tagName === "MAIN") return box({ left: 0, right: 1440 });
        if (this.tagName === "ARTICLE") return box({ left: 40, right: 960 });
        return box({});
      });
      lend("offsetParent", function () {
        return this.parentElement?.closest("main") ?? null;
      });
      lend("offsetWidth", function () {
        return this.getAttribute("role") === "toolbar" ? 105 : 0;
      });
      lend("offsetHeight", function () {
        return this.getAttribute("role") === "toolbar" ? 40 : 0;
      });
      observed = null;
      globalThis.ResizeObserver = class {
        constructor(callback: () => void) {
          observed = callback;
        }
        observe() {}
        unobserve() {}
        disconnect() {
          observed = null;
        }
      } as unknown as typeof ResizeObserver;
    });
    afterEach(() => {
      for (const [name, descriptor] of lent.splice(0).reverse()) {
        if (descriptor) {
          Object.defineProperty(HTMLElement.prototype, name, descriptor);
        } else {
          Reflect.deleteProperty(HTMLElement.prototype, name);
        }
      }
      globalThis.ResizeObserver = savedObserver;
    });

    it("stays inside the page column, never over the threads beside it", async () => {
      serve([]);
      renderApp("/d/eng/reflection/guides/deploying.md");
      await screen.findByText("at");
      select(textNode("Deploys run "), textNode(" night."), 6);
      const toolbar = bar() as HTMLElement;
      // Above the passage, by the gap.
      expect(toolbar.style.top).toBe("254px");
      // Its right edge on the column's: 960 - 105.
      expect(toolbar.style.left).toBe("855px");
    });

    it("goes below a passage with no room above it", async () => {
      Object.assign(sel, { top: 10, bottom: 30, left: 100 });
      serve([]);
      renderApp("/d/eng/reflection/guides/deploying.md");
      await screen.findByText("at");
      select(textNode("Deploys run "), textNode(" night."), 6);
      expect((bar() as HTMLElement).style.top).toBe("36px");
    });

    it("follows the text when the window or the page changes size", async () => {
      serve([]);
      renderApp("/d/eng/reflection/guides/deploying.md");
      await screen.findByText("at");
      select(textNode("Deploys run "), textNode(" night."), 6);
      const toolbar = bar() as HTMLElement;
      Object.assign(sel, { top: 500, bottom: 520, left: 100 });
      act(() => {
        window.dispatchEvent(new Event("resize"));
      });
      expect(toolbar.style.top).toBe("454px");
      expect(toolbar.style.left).toBe("100px");
      // A picture above the passage loads: the page grows, the text moves.
      Object.assign(sel, { top: 700, bottom: 720, left: 120 });
      expect(observed).not.toBeNull();
      act(() => {
        observed?.();
      });
      expect(toolbar.style.top).toBe("654px");
      expect(toolbar.style.left).toBe("120px");
    });
  });

  it("says the bar is there, with its key", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    const status = screen.getByRole("status", { hidden: true });
    select(textNode("Deploys run "), textNode(" night."), 6);
    expect(status).toHaveTextContent("Comment on the selection with Alt+F10");
  });
});

describe("threads on a narrow screen", () => {
  // jsdom has no media queries at all; one is lent for these tests, with a
  // window that can grow past the threads breakpoint.
  const media = window as unknown as { matchMedia?: unknown };
  let wide = false;
  const listeners = new Set<(event: { matches: boolean }) => void>();
  const grow = () => {
    wide = true;
    act(() => {
      for (const listener of listeners) {
        listener({ matches: true });
      }
    });
  };
  beforeEach(() => {
    wide = false;
    listeners.clear();
    // Wide enough for the sidebar, too narrow for the threads column.
    media.matchMedia = (query: string) => ({
      get matches() {
        return wide || !query.includes("80rem");
      },
      media: query,
      addEventListener: (
        _type: string,
        listener: (event: { matches: boolean }) => void,
      ) => {
        listeners.add(listener);
      },
      removeEventListener: (
        _type: string,
        listener: (event: { matches: boolean }) => void,
      ) => {
        listeners.delete(listener);
      },
    });
  });
  afterEach(() => {
    delete media.matchMedia;
  });

  it("shuts the sheet when the window grows past the breakpoint, focus on the column", async () => {
    serve([thread("t-00000001")]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await userEvent.click(
      await screen.findByRole("button", { name: "Show 1 thread" }),
    );
    await screen.findByRole("dialog", { name: "Threads" });
    grow();
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Threads" })).toBeNull();
    });
    const column = screen.getByRole("complementary", { name: "Threads" });
    await waitFor(() => {
      expect(
        within(column).getByRole("heading", { name: "Threads" }),
      ).toHaveFocus();
    });
  });

  it("drops an answer that landed while the sheet was shut", async () => {
    let land: (value: unknown) => void = () => undefined;
    serve([thread("t-00000001")], {
      "/domains/eng/reflection/threads/t-00000001/answer": () =>
        new Promise((resolve) => {
          land = resolve;
        }),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    await userEvent.click(
      await screen.findByRole("button", { name: "Show 1 thread" }),
    );
    const sheet = await screen.findByRole("dialog", { name: "Threads" });
    await userEvent.click(
      within(sheet).getByRole("button", { name: "Answer" }),
    );
    await userEvent.type(
      within(sheet).getByRole("textbox", { name: "Your answer" }),
      "Yes.",
    );
    await userEvent.click(
      within(sheet).getByRole("button", { name: "Answer" }),
    );
    await userEvent.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Threads" })).toBeNull();
    });
    await act(async () => {
      land(thread("t-00000001"));
      await Promise.resolve();
    });
    await userEvent.click(
      screen.getByRole("button", { name: "Show 1 thread" }),
    );
    const again = await screen.findByRole("dialog", { name: "Threads" });
    expect(
      within(again).queryByRole("textbox", { name: "Your answer" }),
    ).toBeNull();
    expect(within(again).getByRole("button", { name: "Answer" })).toBeVisible();
  });

  it("opens the threads from a pill as a sheet, and Escape gives focus back", async () => {
    serve([thread("t-00000001"), thread("t-00000002")]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const pill = await screen.findByRole("button", { name: "Show 2 threads" });
    expect(pill).toHaveTextContent("2 threads");
    expect(screen.queryByRole("complementary", { name: "Threads" })).toBeNull();
    expect(screen.queryByRole("article", { name: /^Thread/ })).toBeNull();
    await userEvent.click(pill);
    const sheet = await screen.findByRole("dialog", { name: "Threads" });
    expect(sheet).toHaveAttribute("aria-modal", "true");
    expect(
      within(sheet).getAllByRole("article", {
        name: "Thread on Night deploys",
      }),
    ).toHaveLength(2);
    expect(
      within(sheet).getByRole("heading", { name: "Threads" }),
    ).toHaveFocus();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Threads" })).toBeNull();
    });
    expect(pill).toHaveFocus();
  });

  it("opens the sheet at the thread whose quote was tapped", async () => {
    serve([
      thread("t-00000001", {
        anchor: {
          section: "7f3a9c2e",
          quote: "after the batch",
          occurrence: 1,
          state: "found",
        },
      }),
      thread("t-00000002"),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByRole("button", { name: "Show 2 threads" });
    await waitFor(() => {
      expect(registry.get(QUOTE_HIGHLIGHT)?.ranges ?? []).toHaveLength(2);
    });
    const { node: words, at } = pageText("after the batch");
    const doc = document as unknown as { caretRangeFromPoint?: unknown };
    doc.caretRangeFromPoint = () => {
      const range = document.createRange();
      range.setStart(words, at + 2);
      range.collapse(true);
      return range;
    };
    try {
      fireEvent.click(words.parentElement as HTMLElement);
    } finally {
      delete doc.caretRangeFromPoint;
    }
    const sheet = await screen.findByRole("dialog", { name: "Threads" });
    const picked = sheet.querySelector("#t-00000001") as HTMLElement;
    await waitFor(() => {
      expect(picked).toHaveFocus();
    });
    expect(picked.parentElement).toHaveAttribute("data-picked");
  });

  it("keeps an answer in progress when the sheet is shut and opened again", async () => {
    serve([thread("t-00000001")]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await userEvent.click(
      await screen.findByRole("button", { name: "Show 1 thread" }),
    );
    const sheet = await screen.findByRole("dialog", { name: "Threads" });
    await userEvent.click(
      within(sheet).getByRole("button", { name: "Answer" }),
    );
    await userEvent.type(
      within(sheet).getByRole("textbox", { name: "Your answer" }),
      "Half a th",
    );
    await userEvent.click(
      within(sheet).getByRole("button", { name: "Close the threads" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Threads" })).toBeNull();
    });
    await userEvent.click(
      screen.getByRole("button", { name: "Show 1 thread" }),
    );
    const again = await screen.findByRole("dialog", { name: "Threads" });
    expect(
      within(again).getByRole("textbox", { name: "Your answer" }),
    ).toHaveValue("Half a th");
  });

  it("keeps a label taken off when the sheet is shut and opened again", async () => {
    serve([thread("t-00000001")]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await userEvent.click(
      await screen.findByRole("button", { name: "Show 1 thread" }),
    );
    const sheet = await screen.findByRole("dialog", { name: "Threads" });
    await userEvent.click(
      within(sheet).getByRole("button", { name: "Answer" }),
    );
    const field = within(sheet).getByRole("textbox", { name: "Your answer" });
    await userEvent.type(field, "note: ");
    await userEvent.click(
      within(sheet).getByRole("button", { name: "Remove the label" }),
    );
    await userEvent.type(field, "Note: plain");
    await userEvent.click(
      within(sheet).getByRole("button", { name: "Close the threads" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Threads" })).toBeNull();
    });
    await userEvent.click(
      screen.getByRole("button", { name: "Show 1 thread" }),
    );
    const again = await screen.findByRole("dialog", { name: "Threads" });
    const kept = within(again).getByRole("textbox", { name: "Your answer" });
    await userEvent.type(kept, " words");
    // Still the person's plain words, never a chip.
    expect(kept).toHaveValue("Note: plain words");
    expect(
      within(again).queryByRole("button", { name: "Remove the label" }),
    ).toBeNull();
  });

  it("comments on a passage selected on the page from inside the sheet", async () => {
    const opened = vi.fn((_path: string, _init?: RequestInit) =>
      thread("t-00000001"),
    );
    serve([], {
      "/domains/eng/reflection/threads": (path, init) => opened(path, init),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    expect(
      screen.getByText(
        "Comment on the selection with Alt+F10 or from the threads",
      ),
    ).toHaveAttribute("role", "status");
    await userEvent.click(screen.getByRole("button", { name: "Show threads" }));
    const sheet = await screen.findByRole("dialog", { name: "Threads" });
    // The polite line lives in the sheet while it is open: the modal hides
    // everything outside it.
    const status = within(sheet).getByRole("status");
    expect(
      within(sheet).queryByText("Select text in the page to comment on it."),
    ).toBeNull();
    await userEvent.click(
      within(sheet).getByRole("button", { name: "Comment" }),
    );
    expect(within(sheet).getByText("Deploys run **at** night")).toBeVisible();
    await userEvent.type(
      within(sheet).getByRole("textbox", { name: "Your comment" }),
      "Still true?",
    );
    await userEvent.click(
      within(
        within(sheet).getByRole("form", { name: "New comment" }),
      ).getByRole("button", { name: "Comment" }),
    );
    await waitFor(() => {
      expect(opened).toHaveBeenCalledTimes(1);
    });
    expect(bodyOf(opened.mock.calls[0]?.[1])).toEqual({
      path: "guides/deploying.md",
      section: "7f3a9c2e",
      quote: "Deploys run **at** night",
      occurrence: 1,
      text: "Still true?",
    });
    await waitFor(() => {
      expect(status).toHaveTextContent("Comment posted.");
    });
  });

  it("opens the sheet with the form from the bar at the selection", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    await userEvent.click(
      screen.getByRole("button", { name: "Comment on selection" }),
    );
    const sheet = await screen.findByRole("dialog", { name: "Threads" });
    expect(within(sheet).getByText("Deploys run **at** night")).toBeVisible();
    await waitFor(() => {
      expect(
        within(sheet).getByRole("textbox", { name: "Your comment" }),
      ).toHaveFocus();
    });
    expect(
      screen.queryByRole("toolbar", { name: "Selection actions" }),
    ).toBeNull();
  });

  it("is not at the selection while the sheet is open", async () => {
    serve([]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    expect(
      screen.getByRole("toolbar", { name: "Selection actions" }),
    ).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Show threads" }));
    await screen.findByRole("dialog", { name: "Threads" });
    // The passage is still selected behind the sheet (jsdom drops it as the
    // sheet takes the focus; a browser keeps it).
    select(textNode("Deploys run "), textNode(" night."), 6);
    expect(
      screen.queryByRole("toolbar", {
        name: "Selection actions",
        hidden: true,
      }),
    ).toBeNull();
  });

  it("opens the sheet at a thread a link names", async () => {
    serve([thread("t-00000001")]);
    renderApp("/d/eng/reflection/guides/deploying.md#t-00000001");
    const sheet = await screen.findByRole("dialog", { name: "Threads" });
    const card = within(sheet).getByRole("article", {
      name: "Thread on Night deploys",
    });
    await waitFor(() => {
      expect(card).toHaveFocus();
    });
  });
});

describe("threads on paper", () => {
  const later = thread("t-00000002", {
    anchor: {
      section: "7f3a9c2e",
      quote: "after the batch",
      occurrence: 1,
      state: "found",
    },
    opened_at: "2026-09-29T10:00:00Z",
    entries: [
      {
        at: "2026-09-29T10:00:00Z",
        kind: "comment",
        author: { name: "ann", agent: true },
        text: "Which batch?",
      },
    ],
  });
  const done = thread("t-00000003", {
    status: "resolved",
    anchor: {
      section: "7f3a9c2e",
      quote: "Deploys run",
      occurrence: 1,
      state: "found",
    },
    entries: [
      {
        at: "2026-09-28T10:00:00Z",
        kind: "comment",
        author: { name: "ann", agent: false },
        text: "Settled long ago.",
      },
    ],
  });

  it("numbers the open threads at the ends of their quotes while printing, and only then", async () => {
    // Listed out of reading order, with a resolved one among them.
    serve([later, thread("t-00000001"), done]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await waitFor(() => {
      expect(registry.get(QUOTE_HIGHLIGHT)?.ranges).toHaveLength(2);
    });
    // On screen the text holds no number, and the column stays off paper.
    expect(document.querySelector(".print-marker")).toBeNull();
    expect(
      screen.getByRole("complementary", { name: "Threads" }).className,
    ).toContain("print:hidden");
    const before = pageText("Deploys run at night, after").node;
    const words = before.data;

    act(() => {
      window.dispatchEvent(new Event("beforeprint"));
    });
    const marks = [...document.querySelectorAll<HTMLElement>(".print-marker")];
    expect(marks.map((mark) => mark.textContent)).toEqual(["1", "2"]);
    // Both quotes end in one paragraph: the numbers follow each other at its
    // end, after everything the page drew in it, passed over by the
    // anchoring and by assistive technology. The text is not cut.
    const paragraph = before.parentElement?.closest("p");
    expect(marks[0]?.parentElement).toBe(paragraph);
    expect(marks[1]?.parentElement).toBe(paragraph);
    expect(paragraph?.lastElementChild).toBe(marks[1]);
    expect(before.data).toBe(words);
    for (const mark of marks) {
      expect(mark).toHaveAttribute("data-anchor-skip");
      expect(mark).toHaveAttribute("aria-hidden", "true");
    }

    // The Notes at the end: open threads only, in the same order.
    const notes = document.querySelector<HTMLElement>(
      'section[aria-label="Notes"]',
    );
    expect(notes).toHaveAttribute("hidden");
    expect(notes).toHaveAttribute("data-print");
    const items = [...(notes?.querySelectorAll("li") ?? [])];
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent(/^1Deploys run at night/);
    expect(items[0]).toHaveTextContent("vera");
    expect(items[0]).toHaveTextContent("Still true?");
    expect(items[1]).toHaveTextContent(/^2after the batch/);
    expect(items[1]).toHaveTextContent("ann (agent)");
    expect(items[1]).toHaveTextContent("Which batch?");
    expect(notes).not.toHaveTextContent("Settled long ago.");

    act(() => {
      window.dispatchEvent(new Event("afterprint"));
    });
    expect(document.querySelector(".print-marker")).toBeNull();
    // The text node the page drew is the same node, with its words.
    const after = pageText("Deploys run at night, after").node;
    expect(after).toBe(before);
    expect(after.data).toBe(words);
  });

  /** A range's boundaries and words, to compare one moment with another. */
  function boundaries(range: Range | undefined) {
    return range === undefined
      ? null
      : {
          startContainer: range.startContainer,
          startOffset: range.startOffset,
          endContainer: range.endContainer,
          endOffset: range.endOffset,
          text: range.toString(),
        };
  }

  it("leaves a reader's selection, the painted quotes and the comment anchor as they were", async () => {
    const opened = vi.fn((_path: string, _init?: RequestInit) =>
      thread("t-00000009"),
    );
    serve([later, thread("t-00000001")], {
      "/domains/eng/reflection/threads": (path, init) => opened(path, init),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    await waitFor(() => {
      expect(registry.get(QUOTE_HIGHLIGHT)?.ranges).toHaveLength(2);
    });
    // A selection inside the paragraph the numbers go to, ending mid-word.
    const at = pageText("Deploys run at night, after");
    const range = document.createRange();
    range.setStart(at.node, at.at + 12);
    range.setEnd(at.node, at.at + 26);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    act(() => {
      document.dispatchEvent(new Event("selectionchange"));
    });
    const selected = () => boundaries(window.getSelection()?.getRangeAt(0));
    const painted = () =>
      (registry.get(QUOTE_HIGHLIGHT)?.ranges ?? []).map(boundaries);
    const selectionBefore = selected();
    const paintedBefore = painted();
    const paintedObjects = registry.get(QUOTE_HIGHLIGHT)?.ranges;
    expect(selectionBefore?.text).toBe("at night, afte");

    // A print that is started and cancelled.
    act(() => {
      window.dispatchEvent(new Event("beforeprint"));
    });
    expect(selected()).toEqual(selectionBefore);
    expect(painted()).toEqual(paintedBefore);
    act(() => {
      window.dispatchEvent(new Event("afterprint"));
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(selected()).toEqual(selectionBefore);
    expect(painted()).toEqual(paintedBefore);
    // Not even repainted: the numbers coming and going are no change.
    expect(registry.get(QUOTE_HIGHLIGHT)?.ranges).toBe(paintedObjects);

    act(() => {
      document.dispatchEvent(new Event("selectionchange"));
    });
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    await userEvent.type(
      screen.getByRole("textbox", { name: "Your comment" }),
      "Why?",
    );
    await userEvent.click(
      within(screen.getByRole("form", { name: "New comment" })).getByRole(
        "button",
        { name: "Comment" },
      ),
    );
    await waitFor(() => {
      expect(opened).toHaveBeenCalledTimes(1);
    });
    expect(bodyOf(opened.mock.calls[0]?.[1])).toMatchObject({
      section: "7f3a9c2e",
      quote: "at night, afte",
      occurrence: 1,
    });
  });

  it("prints the page's web address, not a crystalline:// name", async () => {
    serve([thread("t-00000001")]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await findOnScreen("Still true?");
    const block = document.querySelector<HTMLElement>("header.print-title");
    const address = `${window.location.origin}/d/eng/reflection/guides/deploying.md`;
    expect(block).toHaveTextContent(address);
    expect(block).not.toHaveTextContent("crystalline://");
    expect(
      document.querySelector("style[data-print-page]")?.textContent,
    ).toContain(`@bottom-left { content: "${address}"; }`);
  });
});

describe("conventional comment labels", () => {
  /** Open the comment form on a passage. */
  async function openForm(): Promise<HTMLElement> {
    await screen.findByText("at");
    select(textNode("Deploys run "), textNode(" night."), 6);
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    return screen.getByRole("form", { name: "New comment" });
  }

  function opener() {
    const opened = vi.fn((_path: string, _init?: RequestInit) =>
      thread("t-00000001"),
    );
    serve([], {
      "/domains/eng/reflection/threads": (path, init) => opened(path, init),
    });
    return opened;
  }

  async function send(form: HTMLElement) {
    await userEvent.click(
      within(form).getByRole("button", { name: "Comment" }),
    );
  }

  function labelled(text: string, overrides: Record<string, unknown> = {}) {
    return {
      entries: [
        {
          at: "2026-09-30T10:00:00Z",
          kind: "comment",
          author: { name: "vera", agent: false },
          text,
        },
      ],
      ...overrides,
    };
  }

  it("chooses a label and decorations from the menu and sends the head with the words", async () => {
    const opened = opener();
    renderApp("/d/eng/reflection/guides/deploying.md");
    const form = await openForm();
    await userEvent.click(within(form).getByRole("button", { name: "Label" }));
    const menu = await screen.findByRole("menu");
    // No decorations until a label is chosen.
    expect(
      within(menu).queryByRole("menuitemcheckbox", { name: /blocking/ }),
    ).toBeNull();
    const question = within(menu).getByRole("menuitemradio", {
      name: /question/,
    });
    expect(question).toHaveTextContent("You need an answer.");
    await userEvent.click(question);
    expect(question).toHaveAttribute("aria-checked", "true");
    await userEvent.click(
      within(menu).getByRole("menuitemcheckbox", { name: /^blocking/ }),
    );
    await userEvent.click(
      within(menu).getByRole("menuitemcheckbox", { name: /^non-blocking/ }),
    );
    // Blocking and non-blocking exclude each other.
    expect(
      within(menu).getByRole("menuitemcheckbox", { name: /^blocking/ }),
    ).toHaveAttribute("aria-checked", "false");
    await userEvent.keyboard("{Escape}");
    const field = within(form).getByRole("textbox", { name: "Your comment" });
    await waitFor(() => {
      expect(field).toHaveFocus();
    });
    expect(within(form).getByText("Label:")).toBeInTheDocument();
    // The choice is read out by a status line that was there all along.
    expect(within(form).getByRole("status")).toHaveTextContent(
      "Label: question, non-blocking",
    );
    expect(
      within(form).getByRole("button", { name: "Remove the label" }),
    ).toBeVisible();
    await userEvent.type(field, "Is this still true?{Enter}After the move.");
    expect(field).toHaveValue("Is this still true?\nAfter the move.");
    await send(form);
    await waitFor(() => {
      expect(opened).toHaveBeenCalledTimes(1);
    });
    expect(bodyOf(opened.mock.calls[0]?.[1])).toMatchObject({
      text: "question (non-blocking): Is this still true?\nAfter the move.",
    });
  });

  it("turns a typed head at the very start into the chip", async () => {
    const opened = opener();
    renderApp("/d/eng/reflection/guides/deploying.md");
    const form = await openForm();
    const field = within(form).getByRole("textbox", { name: "Your comment" });
    await userEvent.type(field, "issue (blocking): the window moved");
    expect(field).toHaveValue("the window moved");
    expect(within(form).getByText("blocking")).toBeVisible();
    await send(form);
    await waitFor(() => {
      expect(opened).toHaveBeenCalledTimes(1);
    });
    expect(bodyOf(opened.mock.calls[0]?.[1])).toMatchObject({
      text: "issue (blocking): the window moved",
    });
  });

  it("turns a pasted conventional comment into the chip and its words", async () => {
    opener();
    renderApp("/d/eng/reflection/guides/deploying.md");
    const form = await openForm();
    const field = within(form).getByRole("textbox", { name: "Your comment" });
    await userEvent.click(field);
    await userEvent.paste("suggestion (if-minor): name it\nOr not.");
    expect(field).toHaveValue("name it\nOr not.");
    expect(within(form).getByText("if-minor")).toBeVisible();
  });

  it("leaves a head that does not parse as text, and one typed after the chip is taken off", async () => {
    const opened = opener();
    renderApp("/d/eng/reflection/guides/deploying.md");
    const form = await openForm();
    const field = within(form).getByRole("textbox", { name: "Your comment" });
    await userEvent.type(field, "bug: it breaks");
    expect(field).toHaveValue("bug: it breaks");
    await userEvent.clear(field);
    await userEvent.type(field, "note: ");
    expect(field).toHaveValue("");
    await userEvent.click(
      within(form).getByRole("button", { name: "Remove the label" }),
    );
    expect(within(form).queryByText("Label:")).toBeNull();
    await userEvent.type(field, "Note: plain words");
    expect(field).toHaveValue("Note: plain words");
    await send(form);
    await waitFor(() => {
      expect(opened).toHaveBeenCalledTimes(1);
    });
    expect(bodyOf(opened.mock.calls[0]?.[1])).toMatchObject({
      text: "Note: plain words",
    });
  });

  it("draws a labelled comment as its pill and bold subject, a plain one as it is", async () => {
    serve([
      thread(
        "t-00000001",
        labelled("issue (blocking): the window moved\nSee the runbook."),
      ),
      thread("t-00000002", labelled("Plain words: here.")),
      thread(
        "t-00000003",
        labelled("question (non-blocking): is this still true?"),
      ),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const subject = await screen.findByText("the window moved");
    expect(subject.tagName).toBe("STRONG");
    const pill = subject.parentElement?.querySelector("[data-label-pill]");
    expect(pill).toHaveTextContent("issue");
    expect(pill).toHaveTextContent("blocking");
    // Only blocking gets colour.
    expect(pill?.querySelector("[data-blocking]")).not.toBeNull();
    expect(screen.getByText("See the runbook.")).toBeVisible();
    expect((await findOnScreen("Plain words: here.")).tagName).toBe("P");
    expect(document.querySelectorAll("[data-label-pill]")).toHaveLength(2);
    // A non-blocking pill gets no red dot.
    const calm = screen
      .getByText("is this still true?")
      .parentElement?.querySelector("[data-label-pill]");
    expect(calm).toHaveTextContent("non-blocking");
    expect(calm?.querySelector("[data-blocking]")).toBeNull();
    // The card shows the opening comment's label icon beside its passage,
    // red only for a blocking thread.
    const [first, second, third] = screen.getAllByRole("article", {
      name: "Thread on Night deploys",
    });
    expect(first?.querySelector("blockquote svg")).toHaveClass(
      "text-(--blocking)",
    );
    expect(second?.querySelector("blockquote svg")).toBeNull();
    const quiet = third?.querySelector("blockquote svg");
    expect(quiet).not.toBeNull();
    expect(quiet).not.toHaveClass("text-(--blocking)");
    expect(quiet).toHaveClass("text-muted");
  });

  it("filters to blocking threads behind a switch that is there only when one is open", async () => {
    serve([
      thread("t-00000001", labelled("issue (blocking): the window moved")),
      thread("t-00000002", labelled("question: still true?")),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const toggle = await screen.findByRole("switch", {
      name: "Show blocking threads only",
    });
    expect(toggle).toHaveTextContent("Blocking only");
    expect(screen.getByText("still true?")).toBeVisible();
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "true");
    expect(screen.queryByText("still true?")).toBeNull();
    expect(screen.getByText("the window moved")).toBeVisible();
  });

  it("shows every thread again when a hidden thread's quote is tapped", async () => {
    serve([
      thread(
        "t-00000001",
        labelled("issue (blocking): the window moved", {
          anchor: {
            section: "7f3a9c2e",
            quote: "after the batch",
            occurrence: 1,
            state: "found",
          },
        }),
      ),
      thread("t-00000002", labelled("question: still true?")),
    ]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    const toggle = await screen.findByRole("switch", {
      name: "Show blocking threads only",
    });
    await waitFor(() => {
      expect(registry.get(QUOTE_HIGHLIGHT)?.ranges ?? []).toHaveLength(2);
    });
    await userEvent.click(toggle);
    expect(screen.queryByText("still true?")).toBeNull();
    const { node: words, at } = pageText("Deploys run at night, after");
    const doc = document as unknown as { caretRangeFromPoint?: unknown };
    doc.caretRangeFromPoint = () => {
      const range = document.createRange();
      range.setStart(words, at + 3);
      range.collapse(true);
      return range;
    };
    try {
      fireEvent.click(words.parentElement as HTMLElement);
    } finally {
      delete doc.caretRangeFromPoint;
    }
    expect(toggle).toHaveAttribute("aria-checked", "false");
    const card = await screen.findByText("still true?");
    expect(card.closest("article")?.parentElement).toHaveAttribute(
      "data-picked",
    );
  });

  it("shows every thread again when a comment is posted under Blocking only", async () => {
    const blocking = thread(
      "t-00000001",
      labelled("issue (blocking): the window moved"),
    );
    let posted = false;
    serve([], {
      "/domains/eng/reflection/threads": () => {
        posted = true;
        return thread("t-00000007");
      },
      "/domains/eng/reflections/guides/deploying.md": docWith(() =>
        posted ? [blocking, thread("t-00000007")] : [blocking],
      ),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    const toggle = await screen.findByRole("switch", {
      name: "Show blocking threads only",
    });
    await userEvent.click(toggle);
    select(textNode("Deploys run "), textNode(" night."), 6);
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    await userEvent.type(
      screen.getByRole("textbox", { name: "Your comment" }),
      "Still true?",
    );
    await userEvent.click(
      within(screen.getByRole("form", { name: "New comment" })).getByRole(
        "button",
        { name: "Comment" },
      ),
    );
    await waitFor(() => {
      expect(document.getElementById("t-00000007")).toHaveFocus();
    });
    expect(toggle).toHaveAttribute("aria-checked", "false");
  });

  it("offers no blocking switch on a page without a blocking thread", async () => {
    serve([thread("t-00000001", labelled("question: still true?"))]);
    renderApp("/d/eng/reflection/guides/deploying.md");
    await screen.findByText("still true?");
    expect(
      screen.queryByRole("switch", { name: "Show blocking threads only" }),
    ).toBeNull();
  });

  it("gives a reply the label menu and a resolve none", async () => {
    const answered = vi.fn((_path: string, _init?: RequestInit) =>
      thread("t-00000001"),
    );
    serve([thread("t-00000001")], {
      "/domains/eng/reflection/threads/t-00000001/answer": (path, init) =>
        answered(path, init),
    });
    renderApp("/d/eng/reflection/guides/deploying.md");
    const card = await screen.findByRole("article", {
      name: "Thread on Night deploys",
    });
    await userEvent.click(
      within(card).getByRole("button", { name: "Resolve" }),
    );
    expect(within(card).queryByRole("button", { name: "Label" })).toBeNull();
    await userEvent.click(within(card).getByRole("button", { name: "Cancel" }));
    await userEvent.click(within(card).getByRole("button", { name: "Answer" }));
    expect(within(card).getByRole("button", { name: "Label" })).toBeVisible();
    // A reply starts with no label.
    expect(within(card).queryByText("Label:")).toBeNull();
    await userEvent.type(
      within(card).getByRole("textbox", { name: "Your answer" }),
      "praise: well spotted",
    );
    expect(
      within(card).getByRole("textbox", { name: "Your answer" }),
    ).toHaveValue("well spotted");
    await userEvent.click(within(card).getByRole("button", { name: "Answer" }));
    await waitFor(() => {
      expect(answered).toHaveBeenCalledTimes(1);
    });
    expect(bodyOf(answered.mock.calls[0]?.[1])).toMatchObject({
      text: "praise: well spotted",
    });
  });
});
