/**
 * Both sides of one change, as a reader meets them.
 *
 * The pane is mounted through a small host rather than through the whole app,
 * because the screens that host it are Task 7's dialog and Task 8's page and
 * neither exists yet; the query client and the theme provider around it are
 * the real ones, since the view reads both.
 *
 * What is asserted is the DOM CodeMirror builds and the text in it - never
 * chunk geometry, which jsdom does no layout for.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import { ThemeProvider } from "../theme/ThemeProvider";
import DiffPane from "./DiffPane";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});
const apiMock = vi.mocked(api);

/** A viewport that answers every media query the same way. */
function matchMediaAnswering(matches: boolean) {
  const listeners = new Set<() => void>();
  window.matchMedia = vi.fn().mockImplementation((query: string) => ({
    matches,
    media: query,
    onchange: null,
    addEventListener: (_: string, listener: () => void) =>
      listeners.add(listener),
    removeEventListener: (_: string, listener: () => void) =>
      listeners.delete(listener),
    addListener: () => undefined,
    removeListener: () => undefined,
    dispatchEvent: () => false,
  }));
}

function mount(detail: Record<string, unknown>) {
  apiMock.mockImplementation((path: string) => {
    if (path.startsWith("/domains/eng/changes/")) {
      return Promise.resolve(detail);
    }
    // A call the pane should never make says so, rather than resolving empty.
    return Promise.reject(new Error(`no stub for ${path}`));
  });
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <ThemeProvider>
        <h2 id="pane-heading">notes/a.md</h2>
        <DiffPane domain="eng" path="notes/a.md" headingId="pane-heading" />
      </ThemeProvider>
    </QueryClientProvider>,
  );
}

const MODIFIED = {
  path: "notes/a.md",
  kind: "modified",
  sha: "9f2c",
  binary: false,
  size_before: 8,
  size_after: 8,
  base: "the old rule\n",
  current: "the new rule\n",
  too_large: false,
};

beforeEach(() => {
  apiMock.mockReset();
});
afterEach(() => {
  vi.restoreAllMocks();
});

describe("the diff pane", () => {
  it("is a unified read-only editor by default, labelled by its heading", async () => {
    matchMediaAnswering(false);
    const { container } = mount(MODIFIED);

    const section = await screen.findByRole("region", { name: "notes/a.md" });
    expect(section).toBeInTheDocument();
    await waitFor(() => {
      expect(container.querySelector(".cm-content")).not.toBeNull();
    });
    expect(container.querySelector(".cm-mergeView")).toBeNull();
    expect(container.querySelector(".cm-content")).toHaveAttribute(
      "aria-readonly",
      "true",
    );
    expect(container.textContent).toContain("the new rule");
    expect(screen.getByText("Modified, 8 B to 8 B")).toBeInTheDocument();
  });

  it("is side by side from the large breakpoint", async () => {
    matchMediaAnswering(true);
    const { container } = mount(MODIFIED);

    await waitFor(() => {
      expect(container.querySelector(".cm-mergeView")).not.toBeNull();
    });
    expect(container.querySelectorAll(".cm-content")).toHaveLength(2);
  });

  it("reads an addition and a deletion as one side against nothing", async () => {
    matchMediaAnswering(false);
    const added = mount({
      ...MODIFIED,
      kind: "added",
      base: null,
      size_before: null,
    });
    await waitFor(() => {
      expect(added.container.querySelector(".cm-content")).not.toBeNull();
    });
    expect(added.container.textContent).toContain("the new rule");
    added.unmount();

    const deleted = mount({
      ...MODIFIED,
      kind: "deleted",
      current: null,
      sha: null,
      size_after: null,
    });
    await waitFor(() => {
      expect(deleted.container.querySelector(".cm-content")).not.toBeNull();
    });
    expect(deleted.container.textContent).toContain("the old rule");
  });

  it("says the sizes for a binary file and the CLI hint for a withheld side", async () => {
    matchMediaAnswering(false);
    const binary = mount({
      ...MODIFIED,
      binary: true,
      base: null,
      current: null,
      size_before: 20480,
      size_after: 24576,
    });
    expect(
      await screen.findByText("Changed, 20 KiB to 24 KiB"),
    ).toBeInTheDocument();
    expect(binary.container.querySelector(".cm-content")).toBeNull();
    binary.unmount();

    mount({
      ...MODIFIED,
      too_large: true,
      base: null,
      current: null,
      size_before: 1468006,
      size_after: 1572864,
    });
    expect(
      await screen.findByText("Too large to show here, 1.4 MiB to 1.5 MiB"),
    ).toBeInTheDocument();
    // The domain the pane is about, not the placeholder the rule carries.
    expect(
      screen.getByText("crystalline origin diff eng --path notes/a.md"),
    ).toBeInTheDocument();
  });

  it("shows the problem when the read fails", async () => {
    matchMediaAnswering(false);
    apiMock.mockRejectedValueOnce(new Error("boom"));
    mount(MODIFIED);

    expect(await screen.findByRole("alert")).toHaveTextContent("boom");
  });
});
