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

import { undo } from "@codemirror/commands";
import { EditorView } from "@codemirror/view";
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { Awareness } from "y-protocols/awareness";
import * as Y from "yjs";

import { ApiProblem, api } from "../api/client";
import { TEXT_NAME } from "../collab/provider";
import type { CollabConflict, CollabSession } from "../collab/useCollabSession";
import { useCollabSession } from "../collab/useCollabSession";
import type { Draft } from "../editor/drafts";
import { readDraft } from "../editor/drafts";
import { docText } from "../editor/setup";
import { SAVE_EVENT } from "../editor/useEditorSession";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  soloCollabSession,
  userFixture,
} from "../test/harness";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

// The session hook is mocked wholesale rather than driven through a fake
// socket: what this file is about is the screen's two surfaces, and the hook
// has its own tests next door. `importOriginal` keeps `fileSpace`, which the
// screen imports from the same module, real.
vi.mock("../collab/useCollabSession", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("../collab/useCollabSession")>();
  return { ...actual, useCollabSession: vi.fn() };
});

const apiMock = vi.mocked(api);
const collabMock = vi.mocked(useCollabSession);

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

/** The neighborhood, which is what says where a resolved wikilink landed. */
function graphResponse() {
  return {
    nodes: [
      {
        id: 1,
        domain: "eng",
        permalink: "alpha",
        title: "Alpha",
        status: "stable",
        type: "engram",
      },
      {
        id: 2,
        domain: "eng",
        permalink: "beta",
        title: "Beta",
        status: "stable",
        type: "engram",
      },
    ],
    edges: [{ from: 1, to: 2, rel_type: "links_to" }],
    truncated: false,
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
      // The editor asks for the neighborhood too: it is what turns a
      // reference the index resolved into somewhere the chip can point.
      "/graph": () => graphResponse(),
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

/**
 * A joined room over a real Y.Text, so the binding under test is the real
 * `yCollab` one: a remote edit here is an actual Yjs update, not a prop.
 */
function joinedSession(overrides: Partial<CollabSession> = {}) {
  const doc = new Y.Doc();
  const ytext = doc.getText(TEXT_NAME);
  ytext.insert(0, CONTENT);
  const awareness = new Awareness(doc);
  awareness.setLocalStateField("user", {
    name: "Ada Lovelace",
    color: "#0ea5e9",
    colorLight: "#0ea5e933",
  });
  const flush = vi.fn();
  const session: CollabSession = {
    ...soloCollabSession(),
    mode: "collab",
    ytext,
    awareness,
    epoch: "e1",
    status: "connected",
    participants: [
      { name: "Ada Lovelace", color: "#0ea5e9", self: true },
      { name: "Grace Hopper", color: "#f59e0b", self: false },
    ],
    flush,
    ...overrides,
  };
  collabMock.mockReturnValue(session);
  return { session, doc, ytext, flush };
}

/** The mounted buffer's own view - `view.dom` is where SAVE_EVENT travels. */
function mountedView(content: HTMLElement): EditorView {
  const host = content.closest(".cm-editor");
  const view = host ? EditorView.findFromDOM(host as HTMLElement) : null;
  if (!view) {
    throw new Error("no EditorView is mounted on the buffer");
  }
  return view;
}

/** Every PUT the api mock has seen. */
function puts() {
  return apiMock.mock.calls.filter(([, init]) => init?.method === "PUT");
}

/** Every read of this domain's tree, in order. */
function trees(): string[] {
  return apiMock.mock.calls
    .map(([path]) => path)
    .filter((path) => path.startsWith("/domains/eng/tree"));
}

beforeEach(() => {
  apiMock.mockReset();
  collabMock.mockReset();
  // Every test that is not about the room runs on the solo surface, exactly
  // as the screen behaved before there was a session to join.
  collabMock.mockReturnValue(soloCollabSession());
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
    // Frontmatter is in the buffer too: the whole file is the document. In
    // preview mode the block is behind its summary chip - the form beside the
    // buffer is the metadata surface there - so the plain text of it is what
    // Raw shows, which is the mode that shows the file as written.
    expect(editor.querySelector(".cm-frontmatter-chip")?.textContent).toContain(
      "engram",
    );
    await userEvent.click(screen.getByRole("button", { name: "Raw" }));
    await waitFor(() => {
      expect(screen.getByLabelText("Engram source").textContent).toContain(
        "permalink: alpha",
      );
    });
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
    // The state is visible as well as announced. Asserted as "the pressed
    // face brings its own color and drops the unpressed one" rather than as
    // pixels: accent utilities layered ON TOP of the ghost tier's own color
    // lose to it in the emitted stylesheet, which is a silent failure with
    // no other test signal.
    expect(raw.className).toContain("bg-accent-100");
    expect(raw.className).not.toContain("text-slate-600");
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

  it("inserts a table from the format bar and leaves one undo behind", async () => {
    serveEditor();
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(editor.textContent).toContain("A rule.");
    });

    const bar = screen.getByRole("toolbar", { name: "Formatting" });
    const view = mountedView(editor);
    // The cursor opens at the top of the file, which is inside the
    // frontmatter; an author inserting a table has it down in the prose.
    act(() => {
      view.dispatch({ selection: { anchor: view.state.doc.length } });
    });
    await userEvent.click(
      within(bar).getByRole("button", { name: "Insert table" }),
    );

    // The skeleton landed once - a second copy would be the command
    // dispatching twice - and the caret went back to the buffer, which is
    // where the next thing typed belongs.
    const text = docText(view.state);
    expect(text.split("| Column | Column |")).toHaveLength(2);
    expect(text).toContain("| --- | --- |");
    expect(view.hasFocus).toBe(true);
    await screen.findByText("Unsaved changes");

    // One undo takes the whole block back out. The command tags itself
    // `input` rather than `input.type`, which is what keeps the history from
    // folding it into the typing around it - or splitting it per character.
    act(() => {
      undo(view);
    });
    expect(docText(view.state)).toBe(CONTENT);
  });

  it("keeps a format-bar insertion out of the folded frontmatter", async () => {
    // The mount-time caret sits at position 0, inside the block the summary
    // chip is hiding, and nothing has clicked into the buffer yet - the fold
    // is atomic for cursor MOTION only, so without a guard in the command the
    // table would land between the opening fence and the first key, invisible
    // behind the chip, and the file would stop parsing.
    serveEditor();
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(editor.textContent).toContain("A rule.");
    });
    const view = mountedView(editor);

    await userEvent.click(screen.getByRole("button", { name: "Insert table" }));

    expect(docText(view.state)).toBe(
      CONTENT.replace(
        "---\n\n# Alpha",
        "---\n| Column | Column |\n| --- | --- |\n|  |  |\n\n# Alpha",
      ),
    );
  });

  it("runs format-bar buttons from the keyboard", async () => {
    serveEditor();
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(editor.textContent).toContain("A rule.");
    });
    const bar = screen.getByRole("toolbar", { name: "Formatting" });
    const view = mountedView(editor);
    act(() => {
      view.dispatch({ selection: { anchor: view.state.doc.length } });
    });

    // Ordinary tab stops running on the ordinary button keys: Space and
    // Enter both activate, because nothing here intercepts a key - they are
    // native buttons in a named toolbar.
    within(bar).getByRole("button", { name: "Bulleted list" }).focus();
    await userEvent.keyboard(" ");
    expect(docText(view.state)).toContain("- ");

    within(bar).getByRole("button", { name: "Insert diagram" }).focus();
    await userEvent.keyboard("{Enter}");
    expect(docText(view.state)).toContain("```mermaid");
    expect(view.hasFocus).toBe(true);
  });

  it("keeps a language-tagged fence in the code face while prose is proportional", async () => {
    // The scroller is proportional now, and `tags.monospace` cannot hold a
    // fence that names a language: the nested parser mounts over the body and
    // the highlighter drops the inherited class on the way in. This asserts
    // the layer that does hold it is actually installed on the screen, in the
    // preview branch, rather than only unit-tested next door.
    const fenced = CONTENT.replace(
      "A rule.",
      'A rule.\n\n```json\n{ "answer": 42 }\n```\n',
    );
    serveEditor({
      "/domains/eng/engrams/alpha": () => detailResponse({ content: fenced }),
    });
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(editor.textContent).toContain('{ "answer": 42 }');
    });
    const body = Array.from(editor.querySelectorAll(".cm-line")).find(
      (line) => line.textContent === '{ "answer": 42 }',
    );
    expect(body?.className).toContain("cm-fence-mono");
    // And in Raw mode it is gone, because the whole buffer is mono there:
    // the face belongs to the preview layer, not to the document.
    await userEvent.click(screen.getByRole("button", { name: "Raw" }));
    await waitFor(() => {
      const raw = Array.from(
        screen.getByLabelText("Engram source").querySelectorAll(".cm-line"),
      ).find((line) => line.textContent === '{ "answer": 42 }');
      expect(raw?.className).not.toContain("cm-fence-mono");
    });
  });

  it("draws a resolved wikilink as a chip and hands the brackets back raw", async () => {
    const linked = CONTENT.replace("A rule.", "A rule about [[Beta]].");
    serveEditor({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({
          content: linked,
          links: [
            {
              line: 9,
              resolved: true,
              target: { domain: null, target: "Beta" },
            },
          ],
        }),
    });
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    // The completion is installed on the buffer whatever the toggle says.
    expect(editor).toHaveAttribute("aria-autocomplete", "list");
    // The chip appears once the neighborhood lands, which is what turns the
    // resolved reference into a place.
    await waitFor(() => {
      expect(editor.querySelector(".cm-wikilink-resolved")?.textContent).toBe(
        "Beta",
      );
    });
    expect(editor.textContent).not.toContain("[[Beta]]");

    // Raw is the file as written, brackets included.
    await userEvent.click(screen.getByRole("button", { name: "Raw" }));
    await waitFor(() => {
      expect(screen.getByLabelText("Engram source").textContent).toContain(
        "[[Beta]]",
      );
    });
    // And nothing the chips did was an edit.
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText("Saved");
    expect(putBody(0)).toEqual({ content: linked });
  });

  it("assists the frontmatter beside the buffer, writing single lines into it", async () => {
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT"
          ? detailResponse({ checksum: "next111" })
          : detailResponse(),
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    // The form reads the buffer rather than the detail payload.
    expect(await screen.findByLabelText("Status")).toHaveValue("stable");

    const status = screen.getByLabelText("Status");
    await userEvent.clear(status);
    await userEvent.type(status, "draft");
    await userEvent.tab();
    // The edit lands inside the folded block, so what the buffer shows is the
    // chip - and it has to be a CURRENT chip: a fold that mapped its old
    // decoration through the change instead of recomputing would leave the
    // summary saying "stable" over a document that says draft.
    await waitFor(() => {
      expect(
        screen
          .getByLabelText("Engram source")
          .querySelector(".cm-frontmatter-chip")?.textContent,
      ).toContain("draft");
    });
    // And the line itself is in the document, which Raw shows unconditionally.
    await userEvent.click(screen.getByRole("button", { name: "Raw" }));
    await waitFor(() => {
      expect(screen.getByLabelText("Engram source").textContent).toContain(
        "status: draft",
      );
    });

    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText("Saved");
    // One line changed; every other byte of the file is what it was.
    expect(putBody(0)).toEqual({
      content: CONTENT.replace("status: stable", "status: draft"),
    });
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
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
    });
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
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
    });
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    // The header echoes the address the engram now answers at.
    expect(await screen.findByText("sharper-alpha")).toBeInTheDocument();
  });

  it("moves the tree on after a save, so a renamed row stops pointing at nothing", async () => {
    serveEditor({
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT"
          ? detailResponse({ permalink: "sharper-alpha", checksum: "next222" })
          : detailResponse(),
      "/domains/eng/engrams/sharper-alpha": () =>
        detailResponse({ permalink: "sharper-alpha", checksum: "next222" }),
      "/domains/eng/tree": () => ({
        domain: "eng",
        path: "/",
        folders: [],
        engrams: [
          {
            permalink: "alpha",
            title: "Alpha",
            type: "engram",
            status: "stable",
            path: "alpha.md",
          },
        ],
      }),
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    await screen.findByRole("link", { name: "Alpha" });
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
    });
    // The tree is fresh for a minute, so nothing but an invalidation can make
    // it be asked for again while this editor sits still.
    const before = trees().length;

    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    // A save can rename the engram, retitle it or retire it, and all three are
    // what a tree row is drawn from. Without this the sidebar keeps a row
    // pointing at an address that answers 404 until the freshness runs out.
    await waitFor(() => {
      expect(trees().length).toBeGreaterThan(before);
    });
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
    // Counted in Raw mode, where every line of the file is a rendered line:
    // preview folds the frontmatter block behind one chip, and the question
    // here is about the document rather than about what decorates it.
    await userEvent.click(screen.getByRole("button", { name: "Raw" }));
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
    // Raw for the count, for the same reason as the test above.
    await userEvent.click(screen.getByRole("button", { name: "Raw" }));
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
    // Raw for the count, so the folded frontmatter block is rendered as its
    // lines: this is a question about the rebuilt document.
    await userEvent.click(screen.getByRole("button", { name: "Raw" }));
    // One buffer line per "\n" in the server's content: a dispatch that kept
    // splitting on the CRLF-mounted state's own separator would collapse the
    // whole thing onto a single line instead.
    const expectedLines = lfServerContent.split("\n").length;
    expect(
      screen.getByLabelText("Engram source").querySelectorAll(".cm-line"),
    ).toHaveLength(expectedLines);
  });

  it("a buffer rebuilt while raw comes back raw, not silently decorated", async () => {
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
    await userEvent.click(screen.getByRole("button", { name: "Raw" }));
    await waitFor(() => {
      expect(screen.getByLabelText("Engram source").textContent).toContain(
        "# Alpha",
      );
    });
    // The separator differs, so this swap rebuilds the whole state rather
    // than dispatching into it - the path that would drop the compartment.
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
    // Still raw, in the button's state and in the buffer alike.
    expect(screen.getByRole("button", { name: "Raw" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByLabelText("Engram source").textContent).toContain(
      "# Alpha",
    );
    // And the toggle still reaches the rebuilt state.
    await userEvent.click(screen.getByRole("button", { name: "Raw" }));
    await waitFor(() => {
      expect(screen.getByLabelText("Engram source").textContent).not.toContain(
        "# Alpha",
      );
    });
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

  it("hard errors disable saving", async () => {
    serveEditor({
      "/validate": () => ({
        errors: 1,
        findings: [
          {
            rule: "E001",
            severity: "error",
            message: "frontmatter will not parse",
            line: 1,
            fix: null,
          },
        ],
      }),
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    });
    expect(screen.getByText(/block saving/i)).toBeInTheDocument();
  });

  it("hard errors block the keyboard save too, not only the button", async () => {
    const put = vi.fn(() => detailResponse({ checksum: "next111" }));
    serveEditor({
      "/validate": () => ({
        errors: 1,
        findings: [
          {
            rule: "E001",
            severity: "error",
            message: "frontmatter will not parse",
            line: 1,
            fix: null,
          },
        ],
      }),
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT" ? put() : detailResponse(),
    });
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    });
    editor.focus();
    // Mod-S dispatches the same `requestSave` the button's `onClick` calls;
    // the hard-error check lives there, not only in the button's `disabled`
    // attribute, so this exercises the keyboard path rather than the button.
    await userEvent.keyboard("{Control>}s{/Control}");
    expect(put).not.toHaveBeenCalled();
  });

  it("rule warnings never block saving", async () => {
    const put = vi.fn(() => detailResponse({ checksum: "next111" }));
    serveEditor({
      "/validate": () => ({
        errors: 0,
        findings: [
          {
            rule: "T005",
            severity: "warning",
            message: "superseded without successor",
            line: null,
            fix: "add - superseded_by [[Target]]",
          },
        ],
      }),
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT" ? put() : detailResponse(),
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(
        screen.getByText(/superseded without successor/),
      ).toBeInTheDocument();
    });
    // The finding is visible, but it never touched the gate.
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(put).toHaveBeenCalled();
    });
  });

  it("says checking is unavailable rather than promising forever when /validate is refused", async () => {
    serveEditor({
      "/validate": () => {
        throw new ApiProblem(403, "forbidden", "validation is not available");
      },
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(screen.getByText(/unavailable/i)).toBeInTheDocument();
    });
    // No verdict landed at all, hard or otherwise: a dry run that cannot
    // even be asked never blocks a save.
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });

  it("the hard-error gate holds through an in-flight revalidation and lifts only when a clean report lands", async () => {
    // Two `/validate` answers this test controls the timing of, so the
    // second request can be left hanging while the gate is checked.
    let resolveFirst: (value: unknown) => void = () => undefined;
    let resolveSecond: (value: unknown) => void = () => undefined;
    const first = new Promise((resolve) => {
      resolveFirst = resolve;
    });
    const second = new Promise((resolve) => {
      resolveSecond = resolve;
    });
    let calls = 0;
    serveEditor({
      "/validate": () => {
        calls += 1;
        return calls === 1 ? first : second;
      },
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(calls).toBe(1);
    });
    resolveFirst({
      errors: 1,
      findings: [
        {
          rule: "E001",
          severity: "error",
          message: "frontmatter will not parse",
          line: 1,
          fix: null,
        },
      ],
    });
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    });

    // A further edit changes the buffer, which - once the debounce settles
    // - changes the validate query's key and starts a second request that
    // has not answered yet.
    const status = screen.getByLabelText("Status");
    await userEvent.clear(status);
    await userEvent.type(status, "draft");
    await userEvent.tab();
    await waitFor(() => {
      expect(calls).toBe(2);
    });

    // The server never re-checks these rule families on save (the verify
    // ceiling), so this gate is the only enforcement there is: the stale
    // hard-error verdict must hold through the window where the fresh one
    // has not landed, or a click here would save invalid content.
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();

    resolveSecond({ errors: 0, findings: [] });
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
    });
  });
});

describe("the engram editor in a session", () => {
  /** Mount the editor on a joined room and wait for the bound buffer. */
  async function openRoom(overrides: Partial<CollabSession> = {}) {
    const room = joinedSession(overrides);
    serveEditor();
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(editor.textContent).toContain("A rule.");
    });
    return { ...room, editor };
  }

  it("waits on a skeleton while the session is still connecting", async () => {
    collabMock.mockReturnValue({ ...soloCollabSession(), mode: "connecting" });
    serveEditor();
    renderApp("/d/eng/edit/alpha");
    expect(
      await screen.findByLabelText("Connecting the session"),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("Engram source")).not.toBeInTheDocument();
  });

  it("names everybody in the room, the local author included", async () => {
    await openRoom();
    const chips = screen.getByRole("list", { name: /in this session/i });
    expect(chips).toHaveAccessibleName(/Ada Lovelace/);
    expect(chips).toHaveAccessibleName(/Grace Hopper/);
    expect(chips.textContent).toContain("Grace Hopper");
    // The local author is marked rather than listed as a stranger.
    expect(chips.textContent).toContain("you");
  });

  it("the Save button asks the session to flush and never PUTs", async () => {
    const { flush } = await openRoom();
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(flush).toHaveBeenCalled();
    expect(puts()).toHaveLength(0);
  });

  it("the keyboard save in a session flushes and never PUTs", async () => {
    // The exact leak the collab transport exists to prevent: a Mod-S that
    // still ran the solo mutation would PUT the mount-time checksum at a
    // server that is debounce-saving the same engram.
    const { flush, editor } = await openRoom();
    const view = mountedView(editor);
    act(() => {
      view.dom.dispatchEvent(new CustomEvent(SAVE_EVENT));
    });
    expect(flush).toHaveBeenCalled();
    expect(puts()).toHaveLength(0);
  });

  it("a format-bar insertion reaches the shared text exactly once", async () => {
    const { editor, ytext } = await openRoom();
    const view = mountedView(editor);
    act(() => {
      view.dispatch({ selection: { anchor: view.state.doc.length } });
    });
    await userEvent.click(screen.getByRole("button", { name: "Insert table" }));
    // The command is a plain transaction on the bound buffer, so the binding
    // writes it into Y.Text once. A skeleton that had been pushed into the
    // shared text by hand as well would be in here twice.
    expect(ytext.toJSON().split("| Column | Column |")).toHaveLength(2);
    expect(ytext.toJSON()).toBe(docText(view.state));
  });

  it("shows a refused session save in the server's own words", async () => {
    await openRoom({
      saveState: "failed",
      saveDetail: "the file is read only",
    });
    // The server's words verbatim, and announced rather than merely shown.
    const alert = await screen.findByText("the file is read only");
    expect(alert).toHaveAttribute("role", "alert");
  });

  it("says so while the session is writing", async () => {
    await openRoom({ saveState: "pending" });
    expect(screen.getByText("Saving...")).toBeInTheDocument();
  });

  it("raises the room's notice when an outside change was folded in", async () => {
    await openRoom({ mergeNotice: true });
    const notice = await screen.findByText(/folded into this session/i);
    expect(notice).toHaveAttribute("role", "status");
  });

  it("normalizes pasted CRLF text at the end of the document to LF", async () => {
    const { editor, ytext } = await openRoom();
    const view = mountedView(editor);
    const end = view.state.doc.length;
    const pasted = "x\r\ny\r\n";
    act(() => {
      // Shaped like the paste it stands in for, cursor after the insert
      // included: that selection belongs to the longer, unrewritten text, so
      // a filter that handed it back would put it past the end of the
      // document it rewrote and CodeMirror would refuse the transaction.
      view.dispatch({
        changes: { from: end, insert: pasted },
        selection: { anchor: end + pasted.length },
      });
    });
    // Nothing threw, and no stray CR reached the buffer or the shared text -
    // a CR in LF session space would land in every participant's file.
    expect(view.state.doc.toString()).toContain("x\ny\n");
    expect(view.state.doc.toString()).not.toContain("\r");
    expect(ytext.toJSON()).toContain("x\ny\n");
    expect(ytext.toJSON()).not.toContain("\r");
  });

  it("takes a remote line carrying a lone CR exactly as the room sent it", async () => {
    // The normalization above must never touch the binding's own write-back.
    // Rebuilding that transaction drops the annotation y-codemirror.next
    // checks, so its sync plugin would write the remote insert BACK into the
    // shared text: the whole room ends up with the line twice and the buffer
    // and the document never agree again. A lone CR is a real line to
    // receive - the server admits a stray-CR file and broadcasts such lines.
    const { doc, ytext, editor } = await openRoom();
    const view = mountedView(editor);
    act(() => {
      doc.transact(() => {
        ytext.insert(ytext.length, "a stray\rcarriage return\n");
      }, "a remote author");
    });
    await waitFor(() => {
      expect(editor.textContent).toContain("carriage return");
    });
    // One copy in the shared text, and the buffer is that text verbatim.
    expect(ytext.toJSON()).toBe(`${CONTENT}a stray\rcarriage return\n`);
    expect(view.state.doc.toString()).toBe(ytext.toJSON());
  });

  it("shows a remote edit in the frontmatter form beside the buffer", async () => {
    const { doc, ytext, editor } = await openRoom();
    expect(await screen.findByLabelText("Status")).toHaveValue("stable");
    act(() => {
      doc.transact(() => {
        ytext.delete(CONTENT.indexOf("stable"), "stable".length);
        ytext.insert(CONTENT.indexOf("stable"), "draft");
      }, "a remote author");
    });
    // The binding put it in the buffer, and the form reads the buffer: the
    // panel beside the text is a view over what the room agreed on. In the
    // buffer the block is folded, so the remote edit shows up there as a
    // refreshed summary chip - derived from the document, so a stale chip
    // would mean a stale buffer.
    await waitFor(() => {
      expect(
        editor.querySelector(".cm-frontmatter-chip")?.textContent,
      ).toContain("draft");
    });
    expect(await screen.findByLabelText("Status")).toHaveValue("draft");
  });

  it("does not offer a draft that is the room's own text on a CRLF file", async () => {
    // The room is LF space and the stored draft is file space, so a CRLF
    // engram whose draft is byte-identical to what everyone is looking at
    // would otherwise be offered on every mount - and accepting it would
    // dispatch a whole-document rewrite into the shared text.
    localStorage.setItem(
      "fluid.draft.ada.eng/alpha",
      JSON.stringify({
        content: CONTENT.replace(/\n/g, "\r\n"),
        baseChecksum: "3f8a1c05e2",
        savedAt: "2026-08-09T10:00:00Z",
      }),
    );
    await openRoom({ separator: "\r\n" });
    expect(screen.queryByText(/unsaved draft/i)).not.toBeInTheDocument();
  });

  it("says it is reconnecting and grays the room out while the socket is down", async () => {
    await openRoom({ status: "reconnecting" });
    const notice = await screen.findByText(/reconnecting/i);
    // Announced: the socket dropped without anybody asking, and what the
    // author needs to know is that their typing is not being lost.
    expect(notice).toHaveAttribute("role", "status");
    expect(notice.textContent).toMatch(/kept locally/i);
    // The chips are still there - those people are still in the room - but
    // dimmed, because who is where stopped being current the moment the
    // awareness channel went quiet.
    const chips = screen.getByRole("list", { name: /in this session/i });
    expect(chips.className).toContain("opacity");
  });

  it("says nothing about the connection while the room is connected", async () => {
    await openRoom();
    expect(screen.queryByText(/reconnecting/i)).not.toBeInTheDocument();
    const chips = screen.getByRole("list", { name: /in this session/i });
    expect(chips.className).not.toContain("opacity");
  });

  it("prompts on unload while the session still owes a save", async () => {
    await openRoom({ saveState: "pending" });
    const event = new Event("beforeunload", { cancelable: true });
    act(() => {
      window.dispatchEvent(event);
    });
    // The session's own verdict, not the solo dirty flag: in a room the
    // server saves, and "pending" means it has not said it landed yet.
    expect(event.defaultPrevented).toBe(true);
  });

  it("lets an unload through once the session says everything is saved", async () => {
    await openRoom({ saveState: "ok" });
    const event = new Event("beforeunload", { cancelable: true });
    act(() => {
      window.dispatchEvent(event);
    });
    expect(event.defaultPrevented).toBe(false);
  });

  it("lets a session save through the client's own hard errors", async () => {
    // The server's parse refusal is the gate in a room, and it answers on
    // the control channel. Blocking the button while Mod-S and the server's
    // own debounce save anyway would be a lie told by a disabled control.
    const { flush } = joinedSession();
    serveEditor({
      "/validate": () => ({
        errors: 1,
        findings: [
          {
            rule: "E001",
            severity: "error",
            message: "frontmatter will not parse",
            line: 1,
            fix: null,
          },
        ],
      }),
    });
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(screen.getByText(/hard error/i)).toBeInTheDocument();
    });
    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toBeEnabled();
    await userEvent.click(save);
    expect(flush).toHaveBeenCalled();
    expect(puts()).toHaveLength(0);
    // And the wording does not claim a block that is not happening.
    expect(screen.queryByText(/block saving/i)).not.toBeInTheDocument();
  });

  it("offers the pre-gap text as a draft once the room rebuilt on a new epoch", async () => {
    // What the hook writes when a reconnect lands on a restarted daemon:
    // the text as it stood, in file space, under this author's draft key.
    // The rebuilt room syncs the file's own text, so the two differ and the
    // surface's ordinary draft banner is what offers the work back.
    localStorage.setItem(
      "fluid.draft.ada.eng/alpha",
      JSON.stringify({
        content: CONTENT.replace("A rule.", "A rule I was still writing."),
        baseChecksum: "3f8a1c05e2",
        savedAt: "2026-08-09T10:00:00Z",
      }),
    );
    const { ytext } = await openRoom({ epoch: "e2" });
    expect(await screen.findByText(/unsaved draft/i)).toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "Restore draft" }),
    );
    // Restored INTO the room: a draft recovered in a session is an ordinary
    // edit of the shared text, not a private buffer beside it.
    await waitFor(() => {
      expect(ytext.toJSON()).toContain("A rule I was still writing.");
    });
  });

  it("still offers a genuinely different draft on a CRLF file", async () => {
    localStorage.setItem(
      "fluid.draft.ada.eng/alpha",
      JSON.stringify({
        content: CONTENT.replace("A rule.", "A recovered rule.").replace(
          /\n/g,
          "\r\n",
        ),
        baseChecksum: "3f8a1c05e2",
        savedAt: "2026-08-09T10:00:00Z",
      }),
    );
    await openRoom({ separator: "\r\n" });
    expect(await screen.findByText(/unsaved draft/i)).toBeInTheDocument();
  });
});

describe("the engram editor with no session to join", () => {
  it("says it is editing solo once the attempt to join gave up", async () => {
    // The whole Group B surface, plus one quiet line saying why there are no
    // chips: a server without a session route, an old daemon, a proxy that
    // will not upgrade. Quiet on purpose - solo editing is not a failure.
    collabMock.mockReturnValue({
      ...soloCollabSession(),
      mode: "solo",
      status: "failed",
    });
    serveEditor();
    renderApp("/d/eng/edit/alpha");
    const notice = await screen.findByText(/editing solo/i);
    expect(notice).toHaveAttribute("role", "status");
    // And it is the solo surface in full: this tab saves for itself again.
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText("Saved");
    expect(putBody(0)).toEqual({ content: CONTENT });
  });

  it("says nothing about solo when no session was ever attempted", async () => {
    collabMock.mockReturnValue({
      ...soloCollabSession(),
      mode: "solo",
      status: "connecting",
    });
    serveEditor();
    renderApp("/d/eng/edit/alpha");
    await screen.findByLabelText("Engram source");
    expect(screen.queryByText(/editing solo/i)).not.toBeInTheDocument();
  });
});

describe("the engram editor resolving a session conflict", () => {
  /** The room, mid-conflict, with the resolution the room picked recorded. */
  async function openConflict(conflict: CollabConflict) {
    const resolve = vi.fn();
    const room = joinedSession({
      saveState: "conflict",
      saveDetail: conflict.detail,
      conflict,
      resolve,
    });
    serveEditor();
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(editor.textContent).toContain("A rule.");
    });
    return { ...room, resolve, editor };
  }

  const EDIT_CONFLICT: CollabConflict = {
    kind: "edit",
    theirs: CONTENT.replace("A rule.", "Their rule."),
    detail: "an agent rewrote this engram",
  };

  it("pauses saving behind a banner that opens both sides", async () => {
    await openConflict(EDIT_CONFLICT);
    const banner = await screen.findByText(
      "Saving is paused: this engram changed outside the session.",
    );
    // Announced, not merely shown: saving stopped without anybody asking.
    expect(banner.closest("[role='alert']")).not.toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Resolve" }));
    // The server's own words, and both texts, before anybody chooses.
    const view = within(await screen.findByRole("dialog"));
    expect(view.getByText("an agent rewrote this engram")).toBeVisible();
    expect(view.getByText(/Their rule\./)).toBeVisible();
    expect(view.getByText(/A rule\./)).toBeVisible();
  });

  it("keeping the session text resolves as mine and leaves the buffer alone", async () => {
    const { resolve, editor } = await openConflict(EDIT_CONFLICT);
    await userEvent.click(screen.getByRole("button", { name: "Resolve" }));
    await userEvent.click(
      screen.getByRole("button", { name: "Keep the session text" }),
    );
    expect(resolve).toHaveBeenCalledWith("mine");
    // Nothing was thrown away locally: the server applies the choice and the
    // room's text is still the room's text.
    expect(editor.textContent).toContain("A rule.");
    // The dialog closes on the choice; the banner clears when the server says
    // the conflict is over.
    await waitFor(() => {
      expect(
        screen.queryByRole("button", { name: "Keep the session text" }),
      ).not.toBeInTheDocument();
    });
  });

  it("taking the file version snapshots the session text as a draft first", async () => {
    // A box rather than a bare variable: what the assertion needs is what the
    // draft store held AT the moment the choice travelled.
    const seen: { draft: Draft | null } = { draft: null };
    const resolve = vi.fn(() => {
      seen.draft = readDraft("ada", "eng", "alpha");
    });
    joinedSession({
      saveState: "conflict",
      saveDetail: EDIT_CONFLICT.detail,
      conflict: EDIT_CONFLICT,
      resolve,
    });
    serveEditor();
    renderApp("/d/eng/edit/alpha");
    const editor = await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(editor.textContent).toContain("A rule.");
    });
    await userEvent.click(screen.getByRole("button", { name: "Resolve" }));
    await userEvent.click(
      screen.getByRole("button", { name: "Take the file version" }),
    );
    expect(resolve).toHaveBeenCalledWith("theirs");
    // The snapshot is written BEFORE the choice travels, the same order the
    // solo 412 flow uses: the room's text is about to be replaced by theirs.
    expect(seen.draft).not.toBeNull();
    expect(seen.draft?.content).toContain("A rule.");
  });

  it("keep editing leaves the conflict pending and writes nothing", async () => {
    const { resolve } = await openConflict(EDIT_CONFLICT);
    await userEvent.click(screen.getByRole("button", { name: "Resolve" }));
    await userEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    expect(resolve).not.toHaveBeenCalled();
    expect(readDraft("ada", "eng", "alpha")).toBeNull();
    // The banner stands: saving is still suspended and the way back in is
    // still on screen.
    expect(
      await screen.findByText(
        "Saving is paused: this engram changed outside the session.",
      ),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "Resolve" })).toBeVisible();
  });

  it("offers the deletion its own wording and can restore with the room's text", async () => {
    const { resolve } = await openConflict({
      kind: "deleted",
      theirs: null,
      detail: "the file was deleted outside this session",
    });
    await userEvent.click(screen.getByRole("button", { name: "Resolve" }));
    const view = within(await screen.findByRole("dialog"));
    expect(
      view.getByText("This engram's file was deleted outside the session"),
    ).toBeVisible();
    // The room's text is readable while the choice is made, whichever side
    // wins: accepting the deletion gives it up.
    expect(view.getByText(/A rule\./)).toBeVisible();
    await userEvent.click(
      screen.getByRole("button", { name: "Restore with the session text" }),
    );
    expect(resolve).toHaveBeenCalledWith("mine");
  });

  it("an accepted deletion keeps the text as a draft and walks the author out", async () => {
    joinedSession({ closed: true });
    serveEditor({
      "/domains/eng/manifest": () => ({ content: "# eng\n" }),
      "/domains/eng/tree": () => ({ folders: [], engrams: [] }),
      "/domains/eng/tags": () => ({ tags: [] }),
      "/domains/eng/engrams": () => ({ engrams: [], total: 0 }),
    });
    renderApp("/d/eng/edit/alpha");
    expect(
      await screen.findByText(
        "This engram was deleted; your text is kept as a draft",
      ),
    ).toBeInTheDocument();
    // The text survives the engram: the same store a crash would have used.
    const draft = readDraft("ada", "eng", "alpha");
    expect(draft?.content).toContain("A rule.");
    // And the author is not left editing a page that is gone: the notice gets
    // its beat on screen, then the domain page takes over.
    expect(
      await screen.findByRole("heading", { name: "eng" }, { timeout: 3000 }),
    ).toBeInTheDocument();
  });
});
