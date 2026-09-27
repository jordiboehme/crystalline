/**
 * The rename summary under `<StrictMode>`, mounted fresh onto a domain that
 * already has a report waiting for it.
 *
 * `main.tsx` wraps the whole app in `<StrictMode>`, which in development
 * double-invokes every effect right after a component's very first mount:
 * setup, then cleanup, then setup again, synchronously, with no re-render in
 * between. An earlier version of `DomainHome`'s rename-summary slot cleared
 * itself from an effect's CLEANUP, on the assumption that the cleanup would
 * only ever run on a real domain change or a real unmount - a mount that
 * lands directly on the domain a report is already waiting for (this file's
 * own scenario) breaks that assumption: the phantom cleanup fires once,
 * unconditionally, before this page's other queries (`manifest`, `members`,
 * `tags`, `syncStatus`) have even settled, and the very next of their
 * re-renders finds the slot already empty.
 *
 * A separate file, not a case inside `RenameDomainDialog.test.tsx`: proving
 * this needs the query cache seeded with the report BEFORE the app's own
 * `QueryProvider` ever creates it, which needs `../query/client` mocked at
 * the module level - a mock that would otherwise leak into every other test
 * in this suite that shares a file with it.
 */

import { render, screen } from "@testing-library/react";
import { StrictMode } from "react";
import { MemoryRouter } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import App from "../App";
import type { RenameReport } from "../api/admin";
import { renameReportKey } from "../api/admin";
import { api } from "../api/client";
import {
  answersFor,
  domainsResponse,
  meResponse,
  userFixture,
} from "../test/harness";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

/**
 * Seeded straight into the client `createQueryClient` builds, before the
 * app's first render: the same slot `RenameDomainDialog` would have written
 * moments earlier in a real session, standing in for it here since this
 * file mounts fresh rather than living through an actual rename.
 */
const REPORT: RenameReport = {
  domain: "engineering",
  previous: "eng",
  localOnly: false,
  manifestWritten: true,
  manifestDraft: false,
  rewritten: [{ domain: "ops", engrams: 2, references: 3 }],
  leftBehind: [],
  aliases: ["eng"],
  shadows: [],
};

vi.mock("../query/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../query/client")>();
  return {
    ...actual,
    createQueryClient: () => {
      const client = actual.createQueryClient();
      client.setQueryData(renameReportKey("engineering"), REPORT);
      return client;
    },
  };
});

const apiMock = vi.mocked(api);

beforeEach(() => {
  apiMock.mockReset();
});

describe("the rename summary under StrictMode", () => {
  it("survives a fresh mount that already finds a report waiting for this domain", async () => {
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture({ role: "admin" }) }),
        "/domains": domainsResponse,
        "/domains/engineering/manifest": () => ({
          domain: "engineering",
          markdown: "# engineering",
        }),
        "/domains/engineering/tree": () => ({
          domain: "engineering",
          path: "/",
          folders: [],
          engrams: [],
        }),
        "/domains/engineering/engrams": () => ({
          mode: "text",
          total: 0,
          page: 1,
          limit: 50,
          count: 0,
          hits: [],
        }),
        "/vocabulary": () => ({
          domain: "engineering",
          tags: [],
          categories: [],
          relation_types: [],
        }),
        "/domains/engineering/members": () => ({
          owner: null,
          visibility: "shared",
          members: [],
        }),
      }),
    );

    render(
      <StrictMode>
        <MemoryRouter initialEntries={["/d/engineering"]}>
          <App />
        </MemoryRouter>
      </StrictMode>,
    );

    expect(
      await screen.findByRole("heading", { level: 1, name: "engineering" }),
    ).toBeVisible();
    expect(screen.getByText("Renamed from eng.")).toBeVisible();

    // Let this page's OTHER queries settle and force further re-renders -
    // exactly what exposed the bug, since none of them touch the rename
    // summary directly. The summary must still be there once they have.
    await screen.findByRole("heading", { name: "Members" });
    expect(screen.getByText("Renamed from eng.")).toBeVisible();
  });
});
