/**
 * The command palette's key on a Mac: Cmd+K only. Ctrl+K there deletes to the
 * end of the line in a text field, so it stays the field's (R111). The search
 * box names the key as ⌘K. The platform is read once, when the modules load,
 * so it is set before any import.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "MacIntel",
    configurable: true,
  });
});

import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

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

function serve() {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
      "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
      "/search": () => ({
        mode: "title",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
      "/vocabulary": () => ({ tags: [] }),
      "/graph": () => ({ nodes: [], edges: [], truncated: false }),
    }),
  );
}

async function openApp() {
  renderApp("/");
  await screen.findByRole("heading", { name: "Home" });
}

beforeEach(() => {
  apiMock.mockReset();
  serve();
});

describe("the command palette on a Mac", () => {
  it("opens on Cmd+K", async () => {
    const user = userEvent.setup();
    await openApp();
    await user.keyboard("{Meta>}k{/Meta}");
    expect(await screen.findByRole("dialog")).toBeVisible();
  });

  it("stays shut on Ctrl+K, and leaves the key to the text field", async () => {
    const user = userEvent.setup();
    await openApp();
    const field = screen.getByRole("searchbox");
    field.focus();
    const event = new KeyboardEvent("keydown", {
      key: "k",
      ctrlKey: true,
      bubbles: true,
      cancelable: true,
    });
    field.dispatchEvent(event);
    await user.keyboard("{Control>}k{/Control}");
    expect(event.defaultPrevented).toBe(false);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("names the key ⌘K in the search box", async () => {
    await openApp();
    expect(screen.getByText("⌘K", { selector: "kbd" })).toBeInTheDocument();
  });
});
