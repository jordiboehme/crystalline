/**
 * Where a share-link lands.
 *
 * The two states a grant has, at both layout widths: read-only with the
 * server's own reason beside it, and joined, where the buffer takes typing and
 * a save says whose draft it landed in. Read-only is not drawn as a failure -
 * somebody who was invited to look is owed the page and the sentence, not an
 * error.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import { JOIN_KEY_STORAGE } from "../api/draftLinks";
import { LayoutWidthContext } from "../layoutWidth";
import GrantedDraft from "./GrantedDraft";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

const DRAFT = {
  domain: "team",
  path: "plan.md",
  owner: "alice",
  permalink: "plan",
  editable: true,
  reason: null,
  content: "---\ntitle: Plan\n---\n\nWhat alice is drafting.\n",
  checksum: "abc",
  join_key: null,
  joined: null,
};

function draw(fullWidth: boolean) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <LayoutWidthContext
        value={{ fullWidth, toggleFullWidth: () => undefined }}
      >
        <MemoryRouter initialEntries={["/draft/dl_token"]}>
          <Routes>
            <Route path="/draft/:token" element={<GrantedDraft />} />
          </Routes>
        </MemoryRouter>
      </LayoutWidthContext>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  apiMock.mockReset();
  sessionStorage.clear();
});

describe.each([
  { width: "the reading measure", fullWidth: false },
  { width: "full width", fullWidth: true },
])("a granted draft at $width", ({ fullWidth }) => {
  it("opens read-only with the server's reason, and offers no way in", async () => {
    apiMock.mockResolvedValue({
      ...DRAFT,
      editable: false,
      reason: "your access on this domain is viewer",
    });
    draw(fullWidth);
    const buffer = await screen.findByLabelText("alice's draft of plan.md");
    expect(buffer).toHaveAttribute("readonly");
    expect(screen.getByRole("status")).toHaveTextContent(
      "your access on this domain is viewer",
    );
    expect(
      screen.queryByRole("button", { name: "Join this draft" }),
    ).not.toBeInTheDocument();
  });

  it("joins, types and is told whose draft the save landed in", async () => {
    apiMock.mockImplementation((path: string) => {
      if (path === "/draft-links/accept")
        return Promise.resolve(DRAFT as never);
      if (path === "/draft-links/join")
        return Promise.resolve({ ...DRAFT, join_key: "key1" } as never);
      return Promise.resolve({
        ...DRAFT,
        checksum: "def",
        joined: "landed in alice's draft",
      } as never);
    });
    const user = userEvent.setup();
    draw(fullWidth);
    await user.click(
      await screen.findByRole("button", { name: "Join this draft" }),
    );
    const buffer = await screen.findByLabelText("alice's draft of plan.md");
    await waitFor(() => {
      expect(buffer).not.toHaveAttribute("readonly");
    });
    expect(sessionStorage.getItem(JOIN_KEY_STORAGE)).toContain("key1");
    await user.type(buffer, "!");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(
      await screen.findByText("landed in alice's draft"),
    ).toBeInTheDocument();
  });

  it("prints the server's words when the link opens nothing", async () => {
    apiMock.mockRejectedValue(
      new (await import("../api/client")).ApiProblem(
        404,
        "not found",
        "this draft link opens nothing",
      ),
    );
    draw(fullWidth);
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "this draft link opens nothing",
    );
  });
});
