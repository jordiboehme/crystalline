/**
 * The version skew banner: shown only when the server runs a different build
 * than this bundle, and it offers the one fix there is, a reload of the tab.
 */

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { VersionSkewToast } from "./VersionSkewToast";

const auth = vi.hoisted(() => ({ serverVersion: "" }));

vi.mock("../auth/AuthContext", () => ({
  useAuth: () => ({ capabilities: { serverVersion: auth.serverVersion } }),
}));

const originalLocation = window.location;
const reload = vi.fn();

beforeEach(() => {
  reload.mockReset();
  Object.defineProperty(window, "location", {
    configurable: true,
    value: { ...originalLocation, reload },
  });
});

afterEach(() => {
  Object.defineProperty(window, "location", {
    configurable: true,
    value: originalLocation,
  });
});

describe("VersionSkewToast", () => {
  it("shows nothing when the versions match", () => {
    auth.serverVersion = import.meta.env.VITE_APP_VERSION;
    render(<VersionSkewToast />);
    expect(screen.queryByRole("button", { name: "Reload" })).toBeNull();
  });

  it("shows nothing when the server version is unknown", () => {
    auth.serverVersion = "";
    render(<VersionSkewToast />);
    expect(screen.queryByRole("button", { name: "Reload" })).toBeNull();
  });

  it("offers a reload that reloads the current page", async () => {
    auth.serverVersion = "99.9.9-other";
    render(<VersionSkewToast />);

    await userEvent.click(
      await screen.findByRole("button", { name: "Reload" }),
    );

    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("shows the hint under the title", async () => {
    auth.serverVersion = "99.9.9-other";
    render(<VersionSkewToast />);

    await screen.findByRole("button", { name: "Reload" });
    const [hint] = screen.getAllByText(
      "Reload this tab to pick up the matching build.",
    );
    expect(hint).not.toHaveClass("sr-only");
  });

  it("hides the banner on dismiss without reloading", async () => {
    auth.serverVersion = "99.9.9-other";
    render(<VersionSkewToast />);

    await userEvent.click(
      await screen.findByRole("button", { name: "Dismiss" }),
    );

    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "Dismiss" })).toBeNull();
    });
    expect(reload).not.toHaveBeenCalled();
  });
});
