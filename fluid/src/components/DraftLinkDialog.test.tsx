/**
 * Sharing one of your own drafts.
 *
 * Every case runs twice, once at each layout width, and that is not
 * ceremony: the editor drops its right-hand column entirely at full width, so
 * a sharing surface that only worked at the reading measure would be missing
 * for anybody working wide. Running the same expectations under both
 * `LayoutWidthContext` values is what keeps this a dialog rather than a panel
 * that happens to be drawn today.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import { LayoutWidthContext } from "../layoutWidth";
import { DraftLinkDialog } from "./DraftLinkDialog";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

function draw(fullWidth: boolean, onClose = vi.fn()) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return {
    onClose,
    ...render(
      <QueryClientProvider client={client}>
        <LayoutWidthContext
          value={{ fullWidth, toggleFullWidth: () => undefined }}
        >
          <DraftLinkDialog domain="team" path="plan.md" onClose={onClose} />
        </LayoutWidthContext>
      </QueryClientProvider>,
    ),
  };
}

beforeEach(() => {
  apiMock.mockReset();
});

describe.each([
  { width: "the reading measure", fullWidth: false },
  { width: "full width", fullWidth: true },
])("the share dialog at $width", ({ fullWidth }) => {
  it("says nobody holds a link until somebody does", async () => {
    apiMock.mockResolvedValue([]);
    draw(fullWidth);
    expect(
      await screen.findByText("Nobody is holding a link to this draft."),
    ).toBeInTheDocument();
  });

  it("shows the minted link once, and says it is the only time", async () => {
    apiMock.mockImplementation((path: string, init?: RequestInit) =>
      init?.method === "POST"
        ? Promise.resolve({
            id: 7,
            token: "dl_secret",
            path: "plan.md",
          } as never)
        : Promise.resolve([] as never),
    );
    const user = userEvent.setup();
    draw(fullWidth);
    await user.click(
      await screen.findByRole("button", { name: "Create link" }),
    );
    await waitFor(() => {
      expect(screen.getByTestId("minted-link")).toHaveTextContent(
        "/draft/dl_secret",
      );
    });
    expect(
      screen.getByText(
        "Copy this link now. It is readable here and never again.",
      ),
    ).toBeInTheDocument();
  });

  it("names who is holding a link and takes it back", async () => {
    let revoked: string | null = null;
    apiMock.mockImplementation((path: string, init?: RequestInit) => {
      if (init?.method === "DELETE") {
        revoked = path;
        return Promise.resolve(undefined as never);
      }
      const rows = revoked
        ? []
        : [
            {
              id: 3,
              domain: "team",
              path: "plan.md",
              owner: "alice",
              grantee: "bob",
              created_at: "2026-09-01T10:00:00Z",
              expires_at: null,
              revoked_at: null,
            },
          ];
      return Promise.resolve(rows as never);
    });
    const user = userEvent.setup();
    draw(fullWidth);
    expect(await screen.findByText("Held by bob")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Revoke" }));
    await waitFor(() => {
      expect(revoked).toBe("/draft-links/3");
    });
  });

  it("prints the server's own words when minting is refused", async () => {
    apiMock.mockImplementation((path: string, init?: RequestInit) =>
      init?.method === "POST"
        ? Promise.reject(
            new ApiProblem(
              404,
              "not found",
              "you are not holding a draft at that path in this domain",
            ),
          )
        : Promise.resolve([] as never),
    );
    const user = userEvent.setup();
    draw(fullWidth);
    await user.click(
      await screen.findByRole("button", { name: "Create link" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "you are not holding a draft at that path in this domain",
    );
  });
});
