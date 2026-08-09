/**
 * The editor screen: what it loads, who it is offered to, and what a save is
 * allowed to do.
 *
 * The whole file is the document - frontmatter included - because that is what
 * the engine writes back, and the save carries the If-Match token of the
 * version it read. A save that renamed the engram through its frontmatter
 * answers at the new address, and the editor follows it there rather than
 * leaving somebody editing a page that now 404s.
 */

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
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

const CONTENT =
  "---\ntitle: Alpha\npermalink: alpha\nstatus: stable\ntype: engram\n---\n\n# Alpha\n\nA rule.\n";

/** The detail payload, in the engine's own shape. */
function detailResponse(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    permalink: "alpha",
    title: "Alpha",
    url: "crystalline://eng/alpha",
    path: "alpha.md",
    content: CONTENT,
    checksum: "3f8a1c05e2",
    frontmatter: { engram_type: "engram", status: "stable", tags: [] },
    observations: [],
    relations: [],
    links: [],
    ...overrides,
  };
}

function serveEditor(
  routes: Record<string, (path: string, init?: RequestInit) => unknown> = {},
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
      "/domains/eng/engrams/alpha": () => detailResponse(),
      "/validate": () => ({ findings: [], errors: 0 }),
      ...routes,
    }),
  );
}

/** The parsed body of the `index`th PUT the mock has seen, in call order. */
function putBody(index: number): unknown {
  const calls = apiMock.mock.calls.filter(([, init]) => init?.method === "PUT");
  const body = calls[index]?.[1]?.body;
  if (typeof body !== "string") {
    throw new Error(`no PUT body at index ${index}`);
  }
  return JSON.parse(body) as unknown;
}

/** The `If-Match` header of the `index`th PUT the mock has seen. */
function putIfMatch(index: number): string | undefined {
  const calls = apiMock.mock.calls.filter(([, init]) => init?.method === "PUT");
  const headers = calls[index]?.[1]?.headers as
    Record<string, string> | undefined;
  return headers?.["If-Match"];
}

beforeEach(() => {
  apiMock.mockReset();
  localStorage.clear();
});

describe("the engram editor", () => {
  it("loads the exact file text into the buffer", async () => {
    serveEditor();
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(editor.textContent).toContain("A rule.");
    });
    // Frontmatter is in the buffer too: the whole file is the document.
    expect(editor.textContent).toContain("permalink: alpha");
  });

  it("renders live preview and hands the raw text back on demand", async () => {
    serveEditor();
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(editor.textContent).toContain("A rule.");
    });
    // The cursor sits at the top of the document, so the heading line is
    // inactive and its marker is folded away.
    expect(editor.textContent).toContain("Alpha");
    expect(editor.textContent).not.toContain("# Alpha");

    const raw = screen.getByRole("button", { name: "Raw" });
    expect(raw).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(raw);
    expect(raw).toHaveAttribute("aria-pressed", "true");
    // Decorations off: the same buffer, now showing exactly what is in it.
    await waitFor(() => {
      expect(screen.getByLabelText("Engram source").textContent).toContain(
        "# Alpha",
      );
    });

    await userEvent.click(screen.getByRole("button", { name: "Raw" }));
    await waitFor(() => {
      expect(screen.getByLabelText("Engram source").textContent).not.toContain(
        "# Alpha",
      );
    });
    // Nothing the toggle did was an edit: the file is unchanged either way.
    expect(screen.queryByText("Unsaved changes")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText("Saved");
    expect(putBody(0)).toEqual({ content: CONTENT });
  });

  it("is not offered to a viewer", async () => {
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture({ role: "viewer" }) }),
        "/domains": domainsResponse,
      }),
    );
    renderApp("/d/eng/edit/alpha");
    await waitFor(() => {
      expect(screen.queryByLabelText("Engram source")).not.toBeInTheDocument();
    });
  });

  it("saves with the If-Match token and reports it", async () => {
    const put = vi.fn(() => detailResponse({ checksum: "next111" }));
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT" ? put() : detailResponse(),
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(put).toHaveBeenCalled();
    });
    const call = apiMock.mock.calls.find(([, init]) => init?.method === "PUT");
    expect(call?.[1]?.headers).toEqual({ "If-Match": '"3f8a1c05e2"' });
    expect(await screen.findByText("Saved")).toBeInTheDocument();
  });

  it("saves from inside the buffer, on the keyboard", async () => {
    const put = vi.fn(() => detailResponse({ checksum: "next111" }));
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT" ? put() : detailResponse(),
    });
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    editor.focus();
    // Mod is Control off the Mac, which is what the test environment says it
    // is running on.
    await userEvent.keyboard("{Control>}s{/Control}");
    await waitFor(() => {
      expect(put).toHaveBeenCalled();
    });
  });

  it("follows a rename to the engram's new address", async () => {
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT"
          ? detailResponse({ permalink: "sharper-alpha", checksum: "next222" })
          : detailResponse(),
      "/domains/eng/engrams/sharper-alpha": () =>
        detailResponse({ permalink: "sharper-alpha", checksum: "next222" }),
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    // The header echoes the address the engram now answers at.
    expect(await screen.findByText("sharper-alpha")).toBeInTheDocument();
  });

  it("offers a stored draft and restores it", async () => {
    localStorage.setItem(
      "fluid.draft.ada.eng/alpha",
      JSON.stringify({
        content: CONTENT.replace("A rule.", "A recovered rule."),
        baseChecksum: "3f8a1c05e2",
        savedAt: "2026-08-09T10:00:00Z",
      }),
    );
    serveEditor();
    renderApp("/d/eng/edit/alpha");
    expect(await screen.findByText(/unsaved draft/i)).toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "Restore draft" }),
    );
    await waitFor(() => {
      expect(screen.getByLabelText("Engram source").textContent).toContain(
        "A recovered rule.",
      );
    });
  });

  it("restoring a draft rebuilds the buffer when the line separator differs", async () => {
    const crlfContent = CONTENT.replace(/\n/g, "\r\n");
    const lfDraftContent = CONTENT.replace("A rule.", "A recovered rule.");
    localStorage.setItem(
      "fluid.draft.ada.eng/alpha",
      JSON.stringify({
        content: lfDraftContent,
        baseChecksum: "3f8a1c05e2",
        savedAt: "2026-08-09T10:00:00Z",
      }),
    );
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT"
          ? detailResponse({ checksum: "after99" })
          : detailResponse({ content: crlfContent }),
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByText(/unsaved draft/i);
    await userEvent.click(
      screen.getByRole("button", { name: "Restore draft" }),
    );
    await waitFor(() => {
      expect(screen.getByLabelText("Engram source").textContent).toContain(
        "A recovered rule.",
      );
    });
    // One buffer line per "\n" in the draft's own content: a dispatch that
    // kept splitting on the CRLF-mounted state's own separator would
    // collapse the whole thing onto a single line instead.
    const expectedLines = lfDraftContent.split("\n").length;
    expect(
      screen.getByLabelText("Engram source").querySelectorAll(".cm-line"),
    ).toHaveLength(expectedLines);
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText("Saved");
    // What went on the wire is the draft's exact bytes, not a corrupted
    // round trip through the wrong separator.
    expect(putBody(0)).toEqual({ content: lfDraftContent });
  });

  it("restoring a draft with a matching line separator still uses the cheap dispatch", async () => {
    const crlfContent = CONTENT.replace(/\n/g, "\r\n");
    const crlfDraftContent = crlfContent.replace(
      "A rule.\r\n",
      "A recovered rule.\r\n",
    );
    localStorage.setItem(
      "fluid.draft.ada.eng/alpha",
      JSON.stringify({
        content: crlfDraftContent,
        baseChecksum: "3f8a1c05e2",
        savedAt: "2026-08-09T10:00:00Z",
      }),
    );
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT"
          ? detailResponse({ checksum: "after99" })
          : detailResponse({ content: crlfContent }),
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByText(/unsaved draft/i);
    await userEvent.click(
      screen.getByRole("button", { name: "Restore draft" }),
    );
    await waitFor(() => {
      expect(screen.getByLabelText("Engram source").textContent).toContain(
        "A recovered rule.",
      );
    });
    const expectedLines = crlfDraftContent.split("\r\n").length;
    expect(
      screen.getByLabelText("Engram source").querySelectorAll(".cm-line"),
    ).toHaveLength(expectedLines);
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText("Saved");
    expect(putBody(0)).toEqual({ content: crlfDraftContent });
  });

  it("discarding the draft keeps the server text and drops the banner", async () => {
    localStorage.setItem(
      "fluid.draft.ada.eng/alpha",
      JSON.stringify({
        content: "other",
        baseChecksum: "3f8a1c05e2",
        savedAt: "",
      }),
    );
    serveEditor();
    renderApp("/d/eng/edit/alpha");
    await screen.findByText(/unsaved draft/i);
    await userEvent.click(
      screen.getByRole("button", { name: "Discard draft" }),
    );
    await waitFor(() => {
      expect(screen.queryByText(/unsaved draft/i)).not.toBeInTheDocument();
    });
    expect(localStorage.getItem("fluid.draft.ada.eng/alpha")).toBeNull();
  });

  it("clears the draft on a successful save", async () => {
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT"
          ? detailResponse({ checksum: "next111" })
          : detailResponse(),
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    localStorage.setItem(
      "fluid.draft.ada.eng/alpha",
      JSON.stringify({
        content: "mid-edit",
        baseChecksum: "3f8a1c05e2",
        savedAt: "",
      }),
    );
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText("Saved");
    expect(localStorage.getItem("fluid.draft.ada.eng/alpha")).toBeNull();
  });

  function conflictAnswer() {
    return new ApiProblem(
      412,
      "precondition failed",
      "stale edit: engram changed since it was read",
      {
        current_etag: '"srv999"',
        current_content: CONTENT.replace("A rule.", "Somebody else's rule."),
      },
    );
  }

  it("a stale save opens the conflict view with both versions", async () => {
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) => {
        if (init?.method === "PUT") {
          throw conflictAnswer();
        }
        return detailResponse();
      },
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(
      await screen.findByRole("dialog", { name: /someone else saved/i }),
    ).toBeInTheDocument();
    expect(screen.getByText(/Somebody else's rule\./)).toBeInTheDocument();
    // The refusal itself, in the server's words.
    expect(
      screen.getByText(/stale edit: engram changed since it was read/),
    ).toBeInTheDocument();
  });

  it("overwrite retries with the server's current token and keeps my text", async () => {
    let puts = 0;
    const seen: Array<Record<string, string>> = [];
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) => {
        if (init?.method === "PUT") {
          seen.push((init.headers ?? {}) as Record<string, string>);
          puts += 1;
          if (puts === 1) {
            throw conflictAnswer();
          }
          return detailResponse({ checksum: "after99" });
        }
        return detailResponse();
      },
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByRole("dialog", { name: /someone else saved/i });
    await userEvent.click(
      screen.getByRole("button", { name: "Save mine over it" }),
    );
    await screen.findByText("Saved");
    expect(seen[1]).toEqual({ "If-Match": '"srv999"' });
  });

  it("taking the server version snapshots my text as a draft first", async () => {
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) => {
        if (init?.method === "PUT") {
          throw conflictAnswer();
        }
        return detailResponse();
      },
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByRole("dialog", { name: /someone else saved/i });
    await userEvent.click(
      screen.getByRole("button", { name: "Take the server version" }),
    );
    await waitFor(() => {
      expect(screen.getByLabelText("Engram source").textContent).toContain(
        "Somebody else's rule.",
      );
    });
    // Mine is not gone: it is the draft now.
    const draft = localStorage.getItem("fluid.draft.ada.eng/alpha");
    expect(draft).not.toBeNull();
    const parsedDraft = JSON.parse(draft ?? "{}") as { content: string };
    expect(parsedDraft.content).toEqual(expect.stringContaining("A rule."));
  });

  it("taking the server version rebuilds the buffer when the line separator differs", async () => {
    const crlfContent = CONTENT.replace(/\n/g, "\r\n");
    const lfServerContent = CONTENT.replace("A rule.", "Somebody else's rule.");
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) => {
        if (init?.method === "PUT") {
          throw new ApiProblem(
            412,
            "precondition failed",
            "stale edit: engram changed since it was read",
            { current_etag: '"srv999"', current_content: lfServerContent },
          );
        }
        return detailResponse({ content: crlfContent });
      },
    });
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(editor.textContent).toContain("A rule.");
    });
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByRole("dialog", { name: /someone else saved/i });
    await userEvent.click(
      screen.getByRole("button", { name: "Take the server version" }),
    );
    await waitFor(() => {
      expect(screen.getByLabelText("Engram source").textContent).toContain(
        "Somebody else's rule.",
      );
    });
    // One buffer line per "\n" in the server's content: a dispatch that kept
    // splitting on the CRLF-mounted state's own separator would collapse the
    // whole thing onto a single line instead.
    const expectedLines = lfServerContent.split("\n").length;
    expect(
      screen.getByLabelText("Engram source").querySelectorAll(".cm-line"),
    ).toHaveLength(expectedLines);
  });

  it("keep editing closes the dialog without touching the buffer, the draft or the stale token", async () => {
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) => {
        if (init?.method === "PUT") {
          throw conflictAnswer();
        }
        return detailResponse();
      },
    });
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    const beforeText = editor.textContent;
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByRole("dialog", { name: /someone else saved/i });
    expect(localStorage.getItem("fluid.draft.ada.eng/alpha")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    // The buffer is exactly what it was.
    expect(screen.getByLabelText("Engram source").textContent).toBe(beforeText);
    // No draft appeared: cancelling wrote nothing.
    expect(localStorage.getItem("fluid.draft.ada.eng/alpha")).toBeNull();
    // The token never moved: the next save meets the same stale checksum and
    // the same refusal, rather than a fresh If-Match it was never granted.
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByRole("dialog", { name: /someone else saved/i });
    expect(putIfMatch(0)).toBe('"3f8a1c05e2"');
    expect(putIfMatch(1)).toBe('"3f8a1c05e2"');
    expect(putBody(0)).toEqual({ content: CONTENT });
    expect(putBody(1)).toEqual({ content: CONTENT });
  });
});
