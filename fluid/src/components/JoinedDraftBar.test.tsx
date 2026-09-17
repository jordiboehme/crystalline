/**
 * The bar that says whose draft this window is typing in.
 *
 * Drawn at both layout widths, and that is the assertion rather than an
 * incidental: which measure the content is at says nothing about whether
 * somebody is working inside another person's work, so a bar that appeared at
 * one width and not the other would be missing exactly where the screen is
 * fullest.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import { JOIN_KEY_STORAGE } from "../api/draftLinks";
import { LayoutWidthContext } from "../layoutWidth";
import { JoinedDraftBar } from "./JoinedDraftBar";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

const HELD = {
  key: "abc123",
  domain: "team",
  path: "plan.md",
  owner: "alice",
  permalink: "plan",
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
        <JoinedDraftBar />
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
])("the joined-draft bar at $width", ({ fullWidth }) => {
  it("draws nothing when this window is inside nobody's draft", () => {
    draw(fullWidth);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("names whose draft this window is in", () => {
    sessionStorage.setItem(JOIN_KEY_STORAGE, JSON.stringify(HELD));
    draw(fullWidth);
    expect(screen.getByRole("status")).toHaveTextContent(
      "You are working in alice's draft of plan.md",
    );
  });

  it("leaves, and the bar goes with the join", async () => {
    sessionStorage.setItem(JOIN_KEY_STORAGE, JSON.stringify(HELD));
    apiMock.mockResolvedValue(undefined);
    const user = userEvent.setup();
    draw(fullWidth);
    await user.click(screen.getByRole("button", { name: "Leave" }));
    await waitFor(() => {
      expect(screen.queryByRole("status")).not.toBeInTheDocument();
    });
    expect(sessionStorage.getItem(JOIN_KEY_STORAGE)).toBeNull();
    expect(apiMock).toHaveBeenCalledWith(
      "/draft-links/leave",
      expect.objectContaining({ method: "POST" }),
    );
  });
});
