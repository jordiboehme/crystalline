/**
 * The shared session shell, exercised on its own rather than through a
 * screen: the save carries the token the session holds, a landed save clears
 * the draft, and a stored draft that differs from the served text is offered
 * back. The screens' own tests cover what they add on top of this.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import CmEditor from "./CmEditor";
import { readDraft, writeDraft } from "./drafts";
import { baseExtensions, lineSeparatorFor } from "./setup";
import { useEditorSession } from "./useEditorSession";

/** The hook runs queries (the validation gate), so every render needs a
 *  client; retries off so a failing stub settles in one pass. */
function renderHost(host: ReactElement) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>{host}</QueryClientProvider>,
  );
}

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn() };
});
const apiMock = vi.mocked(api);

const CONTENT = "---\ntitle: A\n---\n\nbody\n";

function Host({
  save,
  initialChecksum = "c1",
}: {
  save: (
    content: string,
    token: string,
  ) => Promise<{ content: string; checksum: string }>;
  initialChecksum?: string;
}) {
  const session = useEditorSession({
    initialContent: CONTENT,
    initialChecksum,
    draftUser: "ada",
    draftDomain: "eng",
    draftSlot: "alpha",
    validateDomain: "eng",
    validatePath: "alpha.md",
    save,
    extensionsFor: (content) => [
      ...lineSeparatorFor(content),
      ...baseExtensions(false),
    ],
    ariaLabel: "Test source",
  });
  return (
    <div>
      <CmEditor
        initialDoc={CONTENT}
        extensions={[...lineSeparatorFor(CONTENT), ...baseExtensions(false)]}
        ariaLabel="Test source"
        onReady={session.onReady}
        onDocChanged={session.setBuffer}
      />
      <button
        onClick={session.requestSave}
        disabled={session.saving || session.hardErrors > 0}
      >
        Save
      </button>
      {session.notice && <p role="status">{session.notice.text}</p>}
      {session.offeredDraft && (
        <button onClick={session.restoreDraft}>Restore draft</button>
      )}
      {/* Plain spans, not `<output>`: that element carries an implicit
          `status` role, which would make the notice above ambiguous. */}
      <span data-testid="dirty">{String(session.dirty)}</span>
      <span data-testid="checksum">{session.checksum}</span>
    </div>
  );
}

beforeEach(() => {
  localStorage.clear();
  apiMock.mockReset();
  // /validate answers clean by default.
  apiMock.mockResolvedValue({ findings: [], errors: 0, warnings: 0 });
});

describe("useEditorSession", () => {
  it("saves the buffer with the held token and adopts the receipt", async () => {
    const save = vi
      .fn()
      .mockResolvedValue({ content: CONTENT, checksum: "c2" });
    renderHost(<Host save={save} />);
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(save).toHaveBeenCalledWith(CONTENT, "c1");
    });
    await waitFor(() => {
      expect(screen.getByTestId("checksum")).toHaveTextContent("c2");
    });
    expect(screen.getByRole("status")).toHaveTextContent("Saved");
  });

  it("clears the draft on a successful save", async () => {
    writeDraft("ada", "eng", "alpha", {
      content: "older",
      baseChecksum: "c0",
      savedAt: "2026-08-09T00:00:00Z",
    });
    const save = vi
      .fn()
      .mockResolvedValue({ content: CONTENT, checksum: "c2" });
    renderHost(<Host save={save} />);
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(readDraft("ada", "eng", "alpha")).toBeNull();
    });
  });

  it("offers a stored draft that differs from the served content", () => {
    writeDraft("ada", "eng", "alpha", {
      content: "different text",
      baseChecksum: "c0",
      savedAt: "2026-08-09T00:00:00Z",
    });
    const save = vi.fn();
    renderHost(<Host save={save} />);
    expect(screen.getByRole("button", { name: "Restore draft" })).toBeVisible();
  });
});
