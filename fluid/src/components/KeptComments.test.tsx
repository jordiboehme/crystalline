/**
 * The reader's own kept comments in the threads column: only their own, on
 * this page, with the text to copy and a drop.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { KeptTextEntry } from "../api/kept";
import type { AuthValue } from "../auth/AuthContext";
import { AuthContext } from "../auth/AuthContext";
import { userFixture } from "../test/harness";
import { KeptComments } from "./KeptComments";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn() };
});
const apiMock = vi.mocked(api);

function comment(over: Partial<KeptTextEntry> = {}): KeptTextEntry {
  return {
    id: "c1",
    draft: false,
    kind: "comment",
    path: "guides/a.md",
    file: false,
    kept_at: "2026-10-15T08:00:00Z",
    bytes: 20,
    sender: "Ada Lovelace",
    sender_account: "ada",
    ...over,
  };
}

function serve(entries: KeptTextEntry[] | (() => never)) {
  let listed = entries;
  const dropped: string[] = [];
  apiMock.mockImplementation(async (path: string, init?: RequestInit) => {
    await Promise.resolve();
    if (path === "/domains/eng/kept") {
      if (typeof listed === "function") {
        return listed();
      }
      return { domain: "eng", kept: listed };
    }
    if (path === "/domains/eng/kept/c1") {
      if (init?.method === "DELETE") {
        dropped.push("c1");
        listed = [];
        return undefined;
      }
      return {
        domain: "eng",
        ...comment(),
        text: "Please add the rollback step.",
      };
    }
    throw new ApiProblem(404, "not found", path);
  });
  return dropped;
}

function mount() {
  const value = {
    user: userFixture({ name: "ada", display: "Ada Lovelace" }),
    capabilities: {
      anonymous: false,
      readOnly: false,
      role: "viewer",
      canWrite: false,
      canAdminister: false,
      canShare: false,
      needsSetup: false,
      oauth: false,
      serverVersion: "test",
    },
    login: vi.fn(),
    setup: vi.fn(),
    logout: vi.fn(),
  } satisfies AuthValue;
  render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <AuthContext value={value}>
        <KeptComments domain="eng" path="guides/a.md" />
      </AuthContext>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the reader's kept comments", () => {
  it("shows the reader's own kept comment on this page, its text and a Copy", async () => {
    serve([comment()]);
    mount();
    const note = await screen.findByRole("complementary", {
      name: "Your kept comments",
    });
    expect(note).toHaveTextContent("A comment of yours was kept.");
    await userEvent.click(within(note).getByRole("button", { name: "Show" }));
    const dialog = await screen.findByRole("dialog", { name: "Kept comment" });
    expect(
      await within(dialog).findByLabelText("The kept comment"),
    ).toHaveTextContent("Please add the rollback step.");
    expect(within(dialog).getByRole("button", { name: "Copy" })).toBeVisible();
  });

  it("drops it after a confirm", async () => {
    const dropped = serve([comment()]);
    mount();
    const note = await screen.findByRole("complementary", {
      name: "Your kept comments",
    });
    await userEvent.click(
      within(note).getByRole("button", { name: "Drop the kept comment" }),
    );
    await userEvent.click(
      within(note).getByRole("button", { name: "Drop it" }),
    );
    await waitFor(() => {
      expect(dropped).toEqual(["c1"]);
    });
    await waitFor(() => {
      expect(screen.queryByRole("complementary")).toBeNull();
    });
  });

  it("shows nothing of somebody else's, of another page, or of a text", async () => {
    serve([
      comment({ sender: "ben", sender_account: "ben" }),
      // The same display name, another account: not the reader's.
      comment({ id: "c4", sender_account: "ada2" }),
      // An older kept file with no recorded account: nobody's here.
      comment({ id: "c5", sender_account: null }),
      comment({ id: "c2", path: "guides/b.md" }),
      comment({ id: "c3", kind: "reflection" }),
    ]);
    mount();
    await waitFor(() => {
      expect(apiMock).toHaveBeenCalledWith("/domains/eng/kept");
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByRole("complementary")).toBeNull();
  });

  it("shows nothing, and says nothing, without a right to kept texts", async () => {
    serve(() => {
      throw new ApiProblem(403, "forbidden", "no right");
    });
    mount();
    await waitFor(() => {
      expect(apiMock).toHaveBeenCalledWith("/domains/eng/kept");
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByRole("complementary")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
