/**
 * A tab left open across `crystalline update now` still names the old chunk
 * hashes, so the note's chunk may be gone when the chip is chosen. The
 * failure stays in the note's place as one short line with a reload, and
 * the rest of the page is still there.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api, setCsrfToken } from "../api/client";
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

vi.mock("./UpdateNote", () => {
  throw new TypeError("Failed to fetch dynamically imported module");
});

const apiMock = vi.mocked(api);

beforeEach(() => {
  apiMock.mockReset();
  vi.mocked(setCsrfToken).mockReset();
  localStorage.clear();
});

describe("the update note's chunk", () => {
  it("a failed chunk draws a short reload line in the note and keeps the page", async () => {
    // React and lazyChunk report the failed import; that is expected here.
    vi.spyOn(console, "error").mockImplementation(() => {});
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () =>
          meResponse({
            user: userFixture({ role: "admin" }),
            version: "0.34.0",
            update_available: "0.35.0",
          }),
        "/domains": domainsResponse,
        "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
      }),
    );
    renderApp("/");
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Ada Lovelace" }),
    );
    await user.click(
      await screen.findByRole("menuitem", {
        name: "Crystalline 0.35.0 available",
      }),
    );

    // The note's place holds one short line and a reload, not the page-wide
    // prompt.
    const note = await screen.findByRole("dialog", {
      name: "Crystalline 0.35.0 available",
    });
    expect(note).toHaveTextContent("Reload the page to see the update.");
    const reload = vi.fn();
    vi.stubGlobal("location", { ...window.location, reload });
    await user.click(within(note).getByRole("button", { name: "Reload" }));
    expect(reload).toHaveBeenCalledOnce();
    vi.unstubAllGlobals();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(
      screen.queryByText("This part of Fluid could not load.", {
        exact: false,
      }),
    ).toBeNull();

    // The page stays usable, and the chip is still in the menu.
    expect(
      screen.getByRole("heading", { name: "Home", level: 1 }),
    ).toBeVisible();
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    const avatar = screen.getByRole("button", { name: "Ada Lovelace" });
    await waitFor(() => {
      expect(avatar).toHaveFocus();
    });
    await user.click(avatar);
    expect(
      await screen.findByRole("menuitem", {
        name: "Crystalline 0.35.0 available",
      }),
    ).toBeVisible();
  });
});
