/**
 * The quiet update note (spec S28-S31): a chip in the avatar menu when the
 * probe names a newer version, and the small popover it opens. The server
 * fills the field for an admin only; this side draws it only when the field
 * is a plain `x.y.z`.
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

const apiMock = vi.mocked(api);

/** Signed in as an admin, with the probe's update field as given. */
function serveAdmin(update: string | null | undefined) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({
          user: userFixture({ role: "admin" }),
          version: "0.34.0",
          ...(update === undefined ? {} : { update_available: update }),
        }),
      "/domains": domainsResponse,
      "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
    }),
  );
}

/** Open the avatar menu and wait for its lazy body. */
async function openAvatar(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: "Ada Lovelace" }));
  await screen.findByRole("menuitem", { name: "Maintenance" });
  return screen.getByRole("menu");
}

beforeEach(() => {
  apiMock.mockReset();
  vi.mocked(setCsrfToken).mockReset();
  localStorage.clear();
});

describe("the update chip", () => {
  it("chip_only_when_available", async () => {
    serveAdmin("0.35.0");
    const first = renderApp("/");
    const user = userEvent.setup();
    const menu = await openAvatar(user);
    expect(
      within(menu).getByRole("menuitem", { name: /available$/ }),
    ).toBeVisible();
    first.unmount();

    for (const update of [undefined, null]) {
      serveAdmin(update);
      const view = renderApp("/");
      const again = await openAvatar(user);
      expect(
        within(again).queryByRole("menuitem", { name: /available$/ }),
      ).toBeNull();
      view.unmount();
    }
  });

  it("an_invalid_version_draws_no_chip", async () => {
    const user = userEvent.setup();
    for (const update of ["0.35.0<img src=x>", "v0.35.0", "0.35.0-rc.1"]) {
      serveAdmin(update);
      const view = renderApp("/");
      const menu = await openAvatar(user);
      expect(
        within(menu).queryByRole("menuitem", { name: /available/ }),
      ).toBeNull();
      expect(menu.querySelector("img")).toBeNull();
      expect(menu).not.toHaveTextContent("0.35.0");
      view.unmount();
    }
  });

  it("chip_accessible_name", async () => {
    serveAdmin("0.35.0");
    renderApp("/");
    const user = userEvent.setup();
    const menu = await openAvatar(user);
    const chip = within(menu).getByRole("menuitem", {
      name: "Crystalline 0.35.0 available",
    });
    expect(chip).toHaveTextContent("0.35.0 available");
  });
});

describe("the update popover", () => {
  it("popover_names_both_versions_the_notes_link_and_the_command", async () => {
    serveAdmin("0.35.0");
    renderApp("/");
    const user = userEvent.setup();
    await openAvatar(user);
    await user.click(
      screen.getByRole("menuitem", { name: "Crystalline 0.35.0 available" }),
    );

    const note = await screen.findByRole("dialog", {
      name: "Crystalline 0.35.0 available",
    });
    expect(note).toHaveTextContent(
      "This server runs 0.34.0. 0.35.0 is available.",
    );
    const link = within(note).getByRole("link", { name: /^Release notes/ });
    expect(link).toHaveAttribute(
      "href",
      "https://github.com/jordiboehme/crystalline/releases/tag/v0.35.0",
    );
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
    expect(note).toHaveTextContent("Run on the server:");
    expect(within(note).getByText("crystalline update now").tagName).toBe(
      "CODE",
    );
    expect(note).toHaveTextContent("The update check runs on the server.");

    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    await user.click(
      within(note).getByRole("button", { name: "Copy the command" }),
    );
    expect(writeText).toHaveBeenCalledWith("crystalline update now");
    expect(await within(note).findByRole("status")).toHaveTextContent("Copied");
  });

  it("escape_returns_focus_to_the_avatar", async () => {
    serveAdmin("0.35.0");
    renderApp("/");
    const user = userEvent.setup();
    const avatar = await screen.findByRole("button", { name: "Ada Lovelace" });
    avatar.focus();
    await user.keyboard("{Enter}");
    const chip = await screen.findByRole("menuitem", {
      name: "Crystalline 0.35.0 available",
    });
    chip.focus();
    await user.keyboard("{Enter}");

    const note = await screen.findByRole("dialog", {
      name: "Crystalline 0.35.0 available",
    });
    await waitFor(() => {
      expect(note).toContainElement(document.activeElement as HTMLElement);
    });
    await user.keyboard("{Escape}");

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    await waitFor(() => {
      expect(avatar).toHaveFocus();
    });
  });
});
