/**
 * A kept copy that is a file, not text: the dialog says so in the server's
 * own sentence, and offers no text to copy.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { KeptTextEntry } from "../api/kept";
import { KeptTextDialog } from "./KeptTextDialog";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn() };
});
const apiMock = vi.mocked(api);

const SENTENCE =
  "This kept copy is a file, not text. Download it from the kept list.";

function entry(): KeptTextEntry {
  return {
    id: "f1",
    draft: true,
    kind: "engram",
    path: "assets/deck.png",
    kept_at: "2026-11-09T08:00:00Z",
    bytes: 20,
    sender: null,
    sender_account: null,
  };
}

describe("KeptTextDialog", () => {
  it("says a kept file is a file and where it is", async () => {
    apiMock.mockImplementation(async () => {
      await Promise.resolve();
      throw new ApiProblem(422, "Unprocessable Entity", SENTENCE);
    });
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <QueryClientProvider client={client}>
        <KeptTextDialog domain="team" entry={entry()} onClose={() => {}} />
      </QueryClientProvider>,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(SENTENCE);
    expect(screen.queryByRole("button", { name: "Copy" })).toBeNull();
  });
});
