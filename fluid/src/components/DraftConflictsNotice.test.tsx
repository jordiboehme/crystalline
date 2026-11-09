/**
 * The domain page's notice of this session's own draft conflicts: each one a
 * path that opens the dialog that settles it, and the drafts a merge changed,
 * in plain words. It reads the draft routes, which any writer may reach, and
 * never the sync status, which needs the share role.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import { DraftConflictsNotice } from "./DraftConflictsNotice";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn() };
});
const apiMock = vi.mocked(api);

function entry(path: string, id = "d1234567", kind = "EditEdit") {
  return {
    id,
    path,
    kind,
    moved_from: null,
    other: null,
    file: false,
    draft: true,
  };
}

/** The list route answering `list` until a resolve empties it. */
function serve(list: () => unknown) {
  let answer = list;
  apiMock.mockImplementation(async (path: string, init?: RequestInit) => {
    await Promise.resolve();
    if (path === "/domains/eng/drafts/conflicts") {
      return answer();
    }
    if (path === "/domains/eng/drafts/conflicts/d1234567") {
      return {
        ...entry("notes/a.md"),
        base: "start",
        local: "my draft text",
        upstream: "the team's text",
        note: null,
        detected_at: null,
      };
    }
    if (
      path === "/domains/eng/drafts/conflicts/d1234567/resolve" &&
      init?.method === "POST"
    ) {
      answer = () => ({ conflicts: [], merged: [] });
      return {
        draft: true,
        kept: "mine",
        kept_copy: false,
        remaining: 0,
      };
    }
    throw new ApiProblem(404, "not found", path);
  });
}

function mount() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <DraftConflictsNotice domain="eng" />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the draft conflicts notice", () => {
  it("names the path and opens the dialog", async () => {
    serve(() => ({ conflicts: [entry("notes/a.md")], merged: [] }));
    mount();

    const notice = await screen.findByRole("region", { name: "Your drafts" });
    expect(notice).toHaveTextContent(
      "Your draft of notes/a.md and the team's text do not fit together. Settle it before you share.",
    );
    await userEvent.click(
      within(notice).getByRole("button", { name: "notes/a.md" }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "Settle conflict",
    });
    expect(await within(dialog).findByText("my draft text")).toBeVisible();
    expect(apiMock).toHaveBeenCalledWith(
      "/domains/eng/drafts/conflicts/d1234567",
    );
    // Never the folder's status, which an editor with no share role cannot read.
    expect(
      apiMock.mock.calls.some(([path]) => path === "/domains/eng/sync"),
    ).toBe(false);
  });

  it("counts many, shows eight", async () => {
    serve(() => ({
      conflicts: Array.from({ length: 10 }, (_unused, index) =>
        entry(`notes/${String(index)}.md`, `d000000${String(index)}`),
      ),
      merged: [],
    }));
    mount();

    const notice = await screen.findByRole("region", { name: "Your drafts" });
    expect(notice).toHaveTextContent(
      "10 of your drafts do not fit with the team's text. Settle them before you share.",
    );
    expect(
      within(notice).getAllByRole("button", { name: /^notes\// }),
    ).toHaveLength(8);
    expect(within(notice).getByText("and 2 more")).toBeVisible();
  });

  it("shows nothing with no conflicts and no merges", async () => {
    serve(() => ({ conflicts: [], merged: [] }));
    const { container } = mount();

    await waitFor(() => {
      expect(apiMock).toHaveBeenCalledWith("/domains/eng/drafts/conflicts");
    });
    await Promise.resolve();
    expect(container).toBeEmptyDOMElement();
  });

  it("says a merge in plain words", async () => {
    serve(() => ({ conflicts: [], merged: ["notes/x.md", "notes/y.md"] }));
    mount();

    const notice = await screen.findByRole("region", { name: "Your drafts" });
    expect(
      within(notice).getByText(
        "Your draft of notes/x.md now also has the team's change.",
      ),
    ).toBeVisible();
    expect(
      within(notice).getByText(
        "Your draft of notes/y.md now also has the team's change.",
      ),
    ).toBeVisible();
    expect(within(notice).queryByRole("button")).toBeNull();
  });

  it("says why the list could not be read, in the server's words", async () => {
    const sentence =
      "The record of where your draft of notes/a.md started cannot be read. Discard that draft, or ask whoever runs this server to remove its record, so it is read as a draft from before 0.32.0.";
    serve(() => {
      throw new ApiProblem(422, "unprocessable", sentence);
    });
    mount();

    const notice = await screen.findByRole("region", { name: "Your drafts" });
    expect(within(notice).getByRole("alert")).toHaveTextContent(sentence);
  });

  it("shows nothing for a caller the route refuses", async () => {
    serve(() => {
      throw new ApiProblem(403, "forbidden", "no access");
    });
    const { container } = mount();

    await waitFor(() => {
      expect(apiMock).toHaveBeenCalledWith("/domains/eng/drafts/conflicts");
    });
    await Promise.resolve();
    expect(container).toBeEmptyDOMElement();
  });

  it("empties once the last conflict is settled", async () => {
    serve(() => ({ conflicts: [entry("notes/a.md")], merged: [] }));
    mount();

    const notice = await screen.findByRole("region", { name: "Your drafts" });
    await userEvent.click(
      within(notice).getByRole("button", { name: "notes/a.md" }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "Settle conflict",
    });
    await userEvent.click(
      await within(dialog).findByRole("button", { name: "Keep mine" }),
    );

    await waitFor(() => {
      expect(screen.queryByRole("region", { name: "Your drafts" })).toBeNull();
    });
  });
});
