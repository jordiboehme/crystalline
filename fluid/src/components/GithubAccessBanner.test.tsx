/**
 * The admin banner about access on the domains connected through the GitHub
 * App: a line while GitHub cannot be reached, and one per closed domain with
 * what to do. Only instance admins see it, on every page.
 */

import { screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import type { Role } from "../api/model";
import { formatInstant } from "../format";
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

const STALE = "2026-10-29T08:00:00Z";
const REASON =
  "the GitHub App is not installed on acme/kb: install it there or disconnect the domain";

function serve(role: Role) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "boss", role }) }),
      "/domains": domainsResponse,
      "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
      "/vocabulary": () => ({ tags: [] }),
      "/github/access": () => ({
        stale_since: STALE,
        closed: [{ domain: "kb", repo: "acme/kb", reason: REASON }],
      }),
    }),
  );
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the GitHub access banner", () => {
  it("tells an admin GitHub cannot be reached and why a domain is closed", async () => {
    serve("admin");
    renderApp("/");
    const banner = await screen.findByRole("status", {
      name: "GitHub access",
    });
    expect(banner).toHaveTextContent(
      `GitHub cannot be reached; access uses the last known roles since ${formatInstant(STALE)}.`,
    );
    expect(banner).toHaveTextContent(`kb: ${REASON}`);
  });

  it("shows nothing to anybody else, and never asks", async () => {
    serve("editor");
    renderApp("/");
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await waitFor(() => {
      expect(
        screen.queryByRole("status", { name: "GitHub access" }),
      ).toBeNull();
    });
    expect(apiMock.mock.calls.some(([path]) => path === "/github/access")).toBe(
      false,
    );
  });
});
