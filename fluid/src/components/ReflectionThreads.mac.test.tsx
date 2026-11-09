/**
 * The selection bar's key, said on a Mac: Alt+F10 is Option+F10 there, and a
 * screen reader says the word (R111). The platform is read once, when the
 * modules load, so it is set before any import.
 */

import { afterEach, beforeEach, expect, it, vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "MacIntel",
    configurable: true,
  });
});

import { act, screen } from "@testing-library/react";

import { api } from "../api/client";
import type { Answer } from "../test/harness";
import {
  answersFor,
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

const MARKDOWN =
  "# Deploying\n\n## Night deploys\n<!-- r:7f3a9c2e -->\n\nDeploys run **at** night.\nDeploys run at night, after the batch.\n";

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

it("says the bar is there with Option+F10, and no Alt", async () => {
  serve([]);
  renderApp("/d/eng/reflection/guides/deploying.md");
  await screen.findByText("at");
  const status = screen.getByRole("status", { hidden: true });
  select(textNode("Deploys run "), textNode(" night."), 6);
  expect(status).toHaveTextContent("Comment on the selection with Option+F10");
  expect(status.textContent).not.toContain("Alt");
});
