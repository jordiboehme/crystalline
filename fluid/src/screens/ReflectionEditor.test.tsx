import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, focusManager } from "@tanstack/react-query";
import { useSyncExternalStore } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { yUndoManagerKeymap } from "y-codemirror.next";
import { Awareness } from "y-protocols/awareness";
import * as Y from "yjs";

import { ApiProblem, api } from "../api/client";
import { TEXT_NAME } from "../collab/provider";
import type { CollabSession } from "../collab/useCollabSession";
import { useCollabSession } from "../collab/useCollabSession";
import { addOffer, readDraft, readOffers, writeDraft } from "../editor/drafts";
import { marksOnEditLine } from "../editor/marks/mode";
import { formatInstant } from "../format";
import {
  answersFor,
  meResponse,
  renderApp,
  soloCollabSession,
  userFixture,
} from "../test/harness";
import { typeText } from "../test/markKit";

vi.setConfig({ testTimeout: 15000 });
vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});
vi.mock("../collab/useCollabSession", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("../collab/useCollabSession")>();
  return { ...actual, useCollabSession: vi.fn() };
});
const apiMock = vi.mocked(api);
const sessionMock = vi.mocked(useCollabSession);

const PAGE = "# A\n\n## One\n<!-- r:aaaa1111 -->\n\nFirst.\n";

function room(text: string): CollabSession {
  const doc = new Y.Doc();
  const ytext = doc.getText(TEXT_NAME);
  ytext.insert(0, text);
  return {
    ...soloCollabSession(),
    mode: "collab",
    status: "connected",
    ytext,
    awareness: new Awareness(doc),
    epoch: "e1",
    // What the hook reports after the greeting: the inner path.
    permalink: "guides/a.md",
    live: true,
    resolve: () => true,
  };
}

function onStatus(right: string, state = "on") {
  return {
    domain: "eng",
    enabled: true,
    state,
    home: "local",
    home_url: null,
    right,
    engrams_visible: false,
    pending_engrams: [],
  };
}

function serve(right: string, status: () => unknown = () => onStatus(right)) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "ann", role: "viewer" }) }),
      "/domains": () => ({ behavior: [], domains: [] }),
      "/reflection/domains": () => ({ domains: [] }),
      "/domains/eng/reflection": status,
      "/domains/eng/reflection/tree": () => ({
        domain: "eng",
        pact: { path: "PACT.md", title: "PACT", exists: true, open_threads: 0 },
        nodes: [],
      }),
    }),
  );
}

beforeEach(() => {
  apiMock.mockReset();
  sessionMock.mockReset();
  localStorage.clear();
});

describe("the reflection editor", () => {
  it("builds the page showing marks, first frame included, for a person who has the setting on", async () => {
    localStorage.setItem("fluid-marks-on-edit-line", "on");
    const built: EditorState[] = [];
    const create = EditorState.create.bind(EditorState);
    vi.spyOn(EditorState, "create").mockImplementation((config) => {
      const state = create(config);
      built.push(state);
      return state;
    });
    serve("edit");
    live(room(PAGE));
    const editor = await openEditor();
    const first = built.find((state) =>
      state.doc.toString().includes("First."),
    );
    expect(first?.facet(marksOnEditLine)).toBe(true);
    const host = editor.closest(".cm-editor");
    const view = host ? EditorView.findFromDOM(host as HTMLElement) : null;
    expect(view?.state.facet(marksOnEditLine)).toBe(true);
  });

  it("keeps the bytes and the caret when the marks setting flips, and the next key lands at the caret", async () => {
    serve("edit");
    live(room(PAGE));
    const editor = await openEditor();
    const host = editor.closest(".cm-editor");
    const view = host ? EditorView.findFromDOM(host as HTMLElement) : null;
    if (!view) {
      throw new Error("no EditorView is mounted on the page");
    }
    expect(view.state.facet(marksOnEditLine)).toBe(false);
    const at = view.state.doc.toString().indexOf("First");
    act(() => {
      view.dispatch({ selection: { anchor: at } });
    });
    const before = view.state.doc.toString();

    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Ada Lovelace" }),
    );
    await user.click(
      await screen.findByRole("menuitemcheckbox", {
        name: "Show Markdown marks on the line I edit",
      }),
    );

    await waitFor(() => {
      expect(view.state.facet(marksOnEditLine)).toBe(true);
    });
    expect(view.state.doc.toString()).toBe(before);
    expect(view.state.selection.main.head).toBe(at);

    act(() => {
      typeText(view, "X");
    });
    expect(view.state.doc.toString()).toBe(
      `${before.slice(0, at)}X${before.slice(at)}`,
    );
  });

  it("opens the live room with the ids hidden and Done back to the page", async () => {
    serve("edit");
    sessionMock.mockReturnValue(room(PAGE));
    renderApp("/d/eng/reflection-edit/guides/a.md");
    const editor = await screen.findByRole("textbox", { name: "Page source" });
    expect(editor.textContent).toContain("First.");
    expect(editor.textContent).not.toContain("r:aaaa1111");
    expect(screen.getByRole("link", { name: "Done" })).toHaveAttribute(
      "href",
      "/d/eng/reflection/guides/a.md",
    );
    expect(sessionMock.mock.calls[0]?.[0]).toMatchObject({
      domain: "eng",
      permalink: "reflection:guides/a.md",
      room: { url: "/api/v1/collab-reflection/eng/guides/a.md" },
    });
  });

  it("heads the column with the page's trail, an Editing chip, the people and the save state", async () => {
    serve("edit");
    sessionMock.mockReturnValue({
      ...room("---\ntitle: Deploying\n---\n\n# A\n\nFirst.\n"),
      participants: [
        { name: "ann", color: "#0ea5e9", self: true, agent: false },
        { name: "ben", color: "#10b981", self: false, agent: false },
      ],
    });
    renderApp("/d/eng/reflection-edit/guides/a.md");
    const editor = await screen.findByRole("textbox", { name: "Page source" });
    const header = screen
      .getByRole("navigation", { name: "Breadcrumb" })
      .closest("header") as HTMLElement;
    // The trail names the page by its title, as the reading page does.
    expect(header).toHaveTextContent("Deploying");
    expect(header).toHaveTextContent("Editing");
    expect(
      screen.getByRole("list", { name: "In this session: ann (you), ben" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Saved")).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { level: 1, name: "Editing Deploying" }),
    ).toHaveClass("sr-only");
    // The frontmatter stays as text a person can change: no chip stands in
    // for it, and the page's own title line is there.
    expect(editor.textContent).toContain("title: Deploying");
    expect(editor.textContent).not.toContain("Frontmatter");
    // The engram editor's format bar, Attach included.
    expect(
      screen.getByRole("toolbar", { name: "Formatting" }),
    ).toContainElement(screen.getByRole("button", { name: "Attach a file" }));
  });

  it("wears the engram editor's format bar, with no Raw and no one-button row", async () => {
    serve("edit");
    sessionMock.mockReturnValue(room(PAGE));
    renderApp("/d/eng/reflection-edit/guides/a.md");
    await screen.findByRole("textbox", { name: "Page source" });
    const bar = screen.getByRole("toolbar", { name: "Formatting" });
    expect(
      within(bar)
        .getAllByRole("button")
        .map(
          (button) => button.getAttribute("aria-label") ?? button.textContent,
        ),
    ).toEqual([
      "Bold",
      "Italic",
      "Strikethrough",
      "Inline code",
      "Heading",
      "Bulleted list",
      "Numbered list",
      "Task list",
      "Blockquote",
      "Link",
      "Insert table",
      "Insert",
      "Attach a file",
    ]);
    expect(screen.queryByRole("toolbar", { name: "Insert" })).toBeNull();
    // A Reflection page renders `[[...]]` as plain text, so no Wiki link.
    expect(within(bar).queryByRole("button", { name: "Wiki link" })).toBeNull();
  });

  it("joins a section into the one above only once the person confirms, in one undo step", async () => {
    serve("edit");
    const session = live(room(PAGE));
    const editor = await openEditor();
    const view = EditorView.findFromDOM(
      editor.closest(".cm-editor") as HTMLElement,
    ) as EditorView;
    act(() => {
      view.dispatch({ selection: { anchor: PAGE.indexOf("## One") + 3 } });
    });
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Heading" }));
    await user.click(
      await screen.findByRole("menuitem", { name: "Heading 3" }),
    );
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent(
      "This joins the section with the one above. The agent will see the change and rebuild its sources.",
    );
    expect(session.get().ytext?.toJSON()).toBe(PAGE);
    await user.click(
      within(dialog).getByRole("button", { name: "Join sections" }),
    );
    expect(session.get().ytext?.toJSON()).toBe("# A\n\n### One\n\nFirst.\n");
    const undo = yUndoManagerKeymap.find((binding) => binding.key === "Mod-z");
    act(() => {
      undo?.run?.(view);
    });
    expect(session.get().ytext?.toJSON()).toBe(PAGE);
  });

  it("keeps typing below the heading when a peer's undo puts the joined section's id back", async () => {
    serve("edit");
    const session = live(room(PAGE));
    const editor = await openEditor();
    const view = EditorView.findFromDOM(
      editor.closest(".cm-editor") as HTMLElement,
    ) as EditorView;
    act(() => {
      view.dispatch({ selection: { anchor: PAGE.indexOf("## One") + 3 } });
    });
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Heading" }));
    await user.click(
      await screen.findByRole("menuitem", { name: "Heading 3" }),
    );
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", {
        name: "Join sections",
      }),
    );
    const joined = "# A\n\n### One\n\nFirst.\n";
    const ytext = session.get().ytext as Y.Text;
    expect(ytext.toJSON()).toBe(joined);
    // This person's caret on the line below the heading; then a peer undoes
    // the join, which puts the id line back at the start of that line. The
    // change reaches this buffer through the room, with no user event.
    const below = joined.indexOf("### One\n") + "### One\n".length;
    act(() => {
      view.dispatch({ selection: { anchor: below } });
    });
    act(() => {
      ytext.doc?.transact(() => {
        ytext.insert(below, "<!-- r:aaaa1111 -->\n");
        ytext.delete(joined.indexOf("### One"), 1);
      });
    });
    expect(ytext.toJSON()).toBe(PAGE);
    act(() => {
      view.dispatch(view.state.replaceSelection("x"), {
        userEvent: "input.type",
      });
    });
    expect(ytext.toJSON()).toBe(
      "# A\n\n## One\n<!-- r:aaaa1111 -->\nx\nFirst.\n",
    );
  });

  it("says Saving while the room writes and Offline while the socket is down, one at a time", async () => {
    serve("edit");
    const session = live({
      ...room(PAGE),
      saveState: "pending",
      flush: vi.fn(),
    });
    await openEditor();
    expect(screen.getByText("Saving")).toBeInTheDocument();
    expect(screen.queryByText("Saved")).toBeNull();
    session.set({ status: "reconnecting" });
    expect(
      await screen.findByText("Offline, kept on this device"),
    ).toHaveAttribute("role", "status");
    expect(screen.queryByText("Saving")).toBeNull();
  });

  it("Done waits for a save the room still owes, then leaves", async () => {
    serve("edit");
    const session = live({
      ...room(PAGE),
      saveState: "pending",
      flush: vi.fn(),
    });
    await openEditor();
    await userEvent.click(screen.getByRole("link", { name: "Done" }));
    expect(
      await screen.findByText("Saving before you leave."),
    ).toBeInTheDocument();
    expect(session.get().flush).toHaveBeenCalled();
    expect(
      screen.getByRole("textbox", { name: "Page source" }),
    ).toBeInTheDocument();
    session.set({ saveState: "ok" });
    await waitFor(() => {
      expect(
        screen.queryByRole("textbox", { name: "Page source" }),
      ).not.toBeInTheDocument();
    });
    expect(readDraft("ann", "eng", "reflection:guides/a.md")).toBeNull();
  });

  it("Done keeps the text as a draft and says so when the owed save does not land", async () => {
    serve("edit");
    live({ ...room(PAGE), status: "reconnecting" });
    await openEditor();
    await userEvent.click(screen.getByRole("link", { name: "Done" }));
    expect(
      await screen.findByText(
        "Not saved yet. Your text is kept as a draft on this device.",
        undefined,
        { timeout: 4000 },
      ),
    ).toHaveAttribute("role", "alert");
    expect(readDraft("ann", "eng", "reflection:guides/a.md")?.content).toBe(
      PAGE,
    );
    await waitFor(
      () => {
        expect(
          screen.queryByRole("textbox", { name: "Page source" }),
        ).not.toBeInTheDocument();
      },
      { timeout: 4000 },
    );
  }, 12000);

  it("names the page by a setext title, past a fence that only looks like one", async () => {
    serve("edit");
    sessionMock.mockReturnValue(
      room("```\n# not this\n```\n\nDeploying\n===\n\nFirst.\n"),
    );
    renderApp("/d/eng/reflection-edit/guides/a.md");
    await screen.findByRole("textbox", { name: "Page source" });
    expect(
      screen.getByRole("heading", { level: 1, name: "Editing Deploying" }),
    ).toBeInTheDocument();
  });

  it("Done keeps the text at once when a conflict holds every save", async () => {
    serve("edit");
    const flush = vi.fn();
    live({
      ...room(PAGE),
      saveState: "conflict",
      conflict: { kind: "edit", theirs: "other", detail: "changed" },
      flush,
    });
    await openEditor();
    await userEvent.click(screen.getByRole("link", { name: "Done" }));
    // No wait for a flush that cannot land: the alert is there at once.
    expect(
      screen.getByText(
        "Not saved yet. Your text is kept as a draft on this device.",
      ),
    ).toHaveAttribute("role", "alert");
    expect(flush).not.toHaveBeenCalled();
    expect(readDraft("ann", "eng", "reflection:guides/a.md")?.content).toBe(
      PAGE,
    );
    await waitFor(
      () => {
        expect(
          screen.queryByRole("textbox", { name: "Page source" }),
        ).not.toBeInTheDocument();
      },
      { timeout: 3000 },
    );
  });

  it("kept text from an earlier session is offered back", async () => {
    serve("edit");
    const session = room(PAGE);
    sessionMock.mockReturnValue(session);
    writeDraft("ann", "eng", "reflection:guides/a.md", {
      content: "typed offline\n",
      baseChecksum: "c0",
      savedAt: "2026-09-30T10:00:00Z",
    });
    renderApp("/d/eng/reflection-edit/guides/a.md");
    expect(
      await screen.findByText(
        "Text from an earlier session is kept on this device.",
      ),
    ).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Put it back" }));
    expect(session.ytext?.toJSON()).toBe("typed offline\n");
    // Kept until the room saves it (the next test follows it there).
    expect(readDraft("ann", "eng", "reflection:guides/a.md")?.content).toBe(
      "typed offline\n",
    );
    expect(
      screen.queryByText(
        "Text from an earlier session is kept on this device.",
      ),
    ).toBeNull();
  });

  it("offers nothing back when the kept text is what the room already holds", async () => {
    serve("edit");
    sessionMock.mockReturnValue(room(PAGE));
    writeDraft("ann", "eng", "reflection:guides/a.md", {
      content: PAGE,
      baseChecksum: "c0",
      savedAt: "",
    });
    renderApp("/d/eng/reflection-edit/guides/a.md");
    await screen.findByRole("textbox", { name: "Page source" });
    expect(
      screen.queryByText(
        "Text from an earlier session is kept on this device.",
      ),
    ).toBeNull();
  });

  it("offers nothing back when the kept text differs only by the ids a save added", async () => {
    serve("edit");
    sessionMock.mockReturnValue(room(PAGE));
    writeDraft("ann", "eng", "reflection:guides/a.md", {
      content: "# A\n\n## One\n\nFirst.\n",
      baseChecksum: "",
      savedAt: "",
    });
    renderApp("/d/eng/reflection-edit/guides/a.md");
    await screen.findByRole("textbox", { name: "Page source" });
    expect(
      screen.queryByText(
        "Text from an earlier session is kept on this device.",
      ),
    ).toBeNull();
  });

  it("keeps the text as a draft when the room refuses a save", async () => {
    serve("edit");
    const session = {
      ...room(`${PAGE}typed\n`),
      saveState: "failed" as const,
      saveDetail:
        "the Reflection of 'eng' is not on here any more, so this room's text is not saved; this tab keeps a copy, and if the page closes the server keeps one too",
    };
    sessionMock.mockReturnValue(session);
    renderApp("/d/eng/reflection-edit/guides/a.md");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "the Reflection of 'eng' is not on here any more",
    );
    expect(readDraft("ann", "eng", "reflection:guides/a.md")?.content).toBe(
      `${PAGE}typed\n`,
    );
    // Typing after the refusal still reaches the draft.
    act(() => {
      (session.ytext as Y.Text).insert(
        (session.ytext as Y.Text).length,
        "more\n",
      );
    });
    expect(readDraft("ann", "eng", "reflection:guides/a.md")?.content).toBe(
      `${PAGE}typed\nmore\n`,
    );
  });

  it("says deleted only when the page was deleted", async () => {
    serve("edit");
    sessionMock.mockReturnValue({
      ...room(PAGE),
      saveState: "pending",
      closed: true,
      closedReason: "internal",
    });
    renderApp("/d/eng/reflection-edit/guides/a.md");
    expect(
      await screen.findByText(
        "The session ended; your text is kept as a draft",
      ),
    ).toBeVisible();
    expect(screen.queryByText(/was deleted/)).toBeNull();
  });

  it("says the text was saved when the Reflection was switched off", async () => {
    serve("edit");
    sessionMock.mockReturnValue({
      ...room(PAGE),
      closed: true,
      closedReason: "switched_off",
    });
    renderApp("/d/eng/reflection-edit/guides/a.md");
    expect(
      await screen.findByText(
        "The Reflection was switched off. Your text was saved first.",
      ),
    ).toBeVisible();
    expect(screen.queryByText(/session ended|was deleted/)).toBeNull();
  });

  it("claims no save when the switch-off's last save was refused", async () => {
    serve("edit");
    sessionMock.mockReturnValue({
      ...room(PAGE),
      saveState: "failed",
      saveDetail: "the page could not be written",
      closed: true,
      closedReason: "switched_off",
    });
    renderApp("/d/eng/reflection-edit/guides/a.md");
    expect(
      await screen.findByText(
        "The session ended; your text is kept as a draft",
      ),
    ).toBeVisible();
    expect(screen.queryByText(/saved first/)).toBeNull();
    expect(readDraft("ann", "eng", "reflection:guides/a.md")?.content).toBe(
      PAGE,
    );
  });

  it("says a failed room is failed and offers a retry, with no second save path", async () => {
    serve("edit");
    sessionMock.mockReturnValue(soloCollabSession());
    renderApp("/d/eng/reflection-edit/guides/a.md");
    expect(
      await screen.findByText("Live editing is not available right now."),
    ).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(sessionMock.mock.calls.length).toBeGreaterThan(1);
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
  });

  it("sends a contributor back from PACT.md, which only curators edit", async () => {
    serve("edit");
    sessionMock.mockReturnValue(room(PAGE));
    renderApp("/d/eng/reflection-edit/PACT.md");
    expect(
      await screen.findByText("You can comment on this page, but not edit it."),
    ).toBeVisible();
    expect(sessionMock).not.toHaveBeenCalled();
  });

  it("sends a reader who may only comment back to the page", async () => {
    serve("read");
    sessionMock.mockReturnValue(room(PAGE));
    renderApp("/d/eng/reflection-edit/guides/a.md");
    expect(
      await screen.findByText("You can comment on this page, but not edit it."),
    ).toBeVisible();
    expect(
      screen.getByRole("link", { name: "Back to the page" }),
    ).toHaveAttribute("href", "/d/eng/reflection/guides/a.md");
    expect(sessionMock).not.toHaveBeenCalled();
  });
});

/**
 * A session the test can change while the editor is on screen: the mocked
 * hook reads it through an external store, so a change re-renders the
 * editor the way a control from the server would.
 */
function live(first: CollabSession) {
  let current = first;
  const listeners = new Set<() => void>();
  sessionMock.mockImplementation(() =>
    useSyncExternalStore(
      (listener) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
      () => current,
    ),
  );
  return {
    get: () => current,
    set: (next: Partial<CollabSession>) => {
      act(() => {
        current = { ...current, ...next };
        for (const listener of listeners) {
          listener();
        }
      });
    },
  };
}

function type(session: CollabSession, text: string) {
  act(() => {
    const ytext = session.ytext as Y.Text;
    ytext.insert(ytext.length, text);
  });
}

const KEY = "reflection:guides/a.md";
const OFFER = "reflection-offer:guides/a.md";
const REFUSAL = "the Reflection of 'eng' is not on here any more";

afterEach(() => {
  focusManager.setFocused(undefined);
});

async function openEditor(): Promise<HTMLElement> {
  renderApp("/d/eng/reflection-edit/guides/a.md");
  return screen.findByRole("textbox", { name: "Page source" });
}

describe("the reflection editor keeps what is owed", () => {
  it("keeps following the typing after a refusal, with the server's words up", async () => {
    serve("edit");
    const session = live(room(PAGE));
    await openEditor();
    session.set({ saveState: "failed", saveDetail: REFUSAL });
    // A keystroke makes the hook say "pending"; the server does not repeat
    // the same refusal.
    session.set({ saveState: "pending", saveDetail: null });
    type(session.get(), "more\n");
    expect(readDraft("ann", "eng", KEY)?.content).toBe(`${PAGE}more\n`);
    expect(screen.getByRole("alert")).toHaveTextContent(REFUSAL);
    expect(screen.queryByText("Saving")).toBeNull();
  });

  it("drops its own copy once the room saves again", async () => {
    serve("edit");
    const session = live(room(PAGE));
    await openEditor();
    session.set({ saveState: "failed", saveDetail: REFUSAL });
    expect(readDraft("ann", "eng", KEY)).not.toBeNull();
    session.set({ saveState: "ok", saveDetail: null });
    expect(readDraft("ann", "eng", KEY)).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("keeps the text while the connection is down, and on leaving", async () => {
    serve("edit");
    const session = live(room(PAGE));
    await openEditor();
    session.set({ status: "reconnecting" });
    type(session.get(), "offline\n");
    expect(readDraft("ann", "eng", KEY)?.content).toBe(`${PAGE}offline\n`);
    await userEvent.click(screen.getByRole("link", { name: "Done" }));
    expect(readDraft("ann", "eng", KEY)?.content).toBe(`${PAGE}offline\n`);
  });

  it("keeps no draft of a pending text left on a connected room", async () => {
    // The room already holds it and saves it on its own schedule; a draft
    // here would come back later as a false offer of older text.
    serve("edit");
    const session = live(room(PAGE));
    await openEditor();
    type(session.get(), "typed\n");
    session.set({ saveState: "pending" });
    await userEvent.click(screen.getByRole("link", { name: "Done" }));
    expect(readDraft("ann", "eng", KEY)).toBeNull();
    expect(readDraft("ann", "eng", OFFER)).toBeNull();
  });

  it("keeps the text under a conflict", async () => {
    serve("edit");
    const session = live(room(PAGE));
    await openEditor();
    session.set({
      saveState: "conflict",
      conflict: { kind: "edit", theirs: "x", detail: "d" },
    });
    type(session.get(), "mine\n");
    expect(readDraft("ann", "eng", KEY)?.content).toBe(`${PAGE}mine\n`);
  });

  it("asks before the page unloads while text is owed", async () => {
    serve("edit");
    const session = live(room(PAGE));
    await openEditor();
    const quiet = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(quiet);
    expect(quiet.defaultPrevented).toBe(false);
    session.set({ saveState: "pending" });
    const owed = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(owed);
    expect(owed.defaultPrevented).toBe(true);
  });
});

describe("the reflection editor taking the file version", () => {
  it("keeps the dialog open and says so when the room is not connected yet", async () => {
    serve("edit");
    const resolve = vi.fn(() => false);
    live({
      ...room(`${PAGE}mine\n`),
      saveState: "conflict",
      conflict: { kind: "edit", theirs: PAGE, detail: "d" },
      resolve,
    });
    await openEditor();
    await userEvent.click(screen.getByRole("button", { name: "Resolve" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Take the file version" }),
    );
    expect(resolve).toHaveBeenCalledWith("theirs");
    expect(within(dialog).getByRole("alert")).toHaveTextContent(
      "Not connected yet. Try again in a moment.",
    );
    expect(screen.getByRole("dialog")).toBeVisible();
  });

  it("keeps this tab's text as an offer that the file version does not overwrite", async () => {
    serve("edit");
    const session = live({
      ...room(`${PAGE}mine\n`),
      saveState: "conflict",
      conflict: { kind: "edit", theirs: PAGE, detail: "d" },
    });
    await openEditor();
    await userEvent.click(screen.getByRole("button", { name: "Resolve" }));
    await userEvent.click(
      screen.getByRole("button", { name: "Take the file version" }),
    );
    // The room converges on theirs and heals.
    act(() => {
      const ytext = session.get().ytext as Y.Text;
      ytext.delete(0, ytext.length);
      ytext.insert(0, PAGE);
    });
    session.set({ saveState: "ok", conflict: null });
    expect(readDraft("ann", "eng", OFFER)?.content).toBe(`${PAGE}mine\n`);
  });

  it("still keeps a copy when a refusal follows", async () => {
    serve("edit");
    const session = live({
      ...room(`${PAGE}mine\n`),
      saveState: "conflict",
      conflict: { kind: "edit", theirs: PAGE, detail: "d" },
    });
    await openEditor();
    await userEvent.click(screen.getByRole("button", { name: "Resolve" }));
    await userEvent.click(
      screen.getByRole("button", { name: "Take the file version" }),
    );
    act(() => {
      const ytext = session.get().ytext as Y.Text;
      ytext.delete(0, ytext.length);
      ytext.insert(0, PAGE);
    });
    session.set({ saveState: "ok", conflict: null });
    session.set({ saveState: "failed", saveDetail: REFUSAL });
    type(session.get(), "after\n");
    expect(readDraft("ann", "eng", KEY)?.content).toBe(`${PAGE}after\n`);
    expect(readDraft("ann", "eng", OFFER)?.content).toBe(`${PAGE}mine\n`);
  });
});

describe("the reflection editor's older offer", () => {
  for (const choice of ["Discard", "Put it back"]) {
    it(`leaves text set aside after the offer appeared when "${choice}" is pressed`, async () => {
      serve("edit");
      const session = live({
        ...room(`${PAGE}mine\n`),
        saveState: "conflict",
        conflict: { kind: "edit", theirs: PAGE, detail: "d" },
      });
      writeDraft("ann", "eng", KEY, {
        content: "older\n",
        baseChecksum: "",
        savedAt: "2026-09-30T10:00:00Z",
      });
      await openEditor();
      await userEvent.click(screen.getByRole("button", { name: "Resolve" }));
      await userEvent.click(
        screen.getByRole("button", { name: "Take the file version" }),
      );
      act(() => {
        const ytext = session.get().ytext as Y.Text;
        ytext.delete(0, ytext.length);
        ytext.insert(0, PAGE);
      });
      session.set({ saveState: "ok", conflict: null });
      await userEvent.click(screen.getByRole("button", { name: choice }));
      expect(readDraft("ann", "eng", OFFER)?.content).toBe(`${PAGE}mine\n`);
    });
  }
});

describe("the reflection editor's offer history", () => {
  const at = (hour: string, content: string) => ({
    content,
    baseChecksum: "",
    savedAt: `2026-10-16T${hour}:00:00Z`,
  });

  it("keeps an unanswered offer when a newer copy comes, and lists both by time", async () => {
    serve("edit");
    const session = live(room(PAGE));
    addOffer("ann", "eng", OFFER, at("08", "first try\n"));
    writeDraft("ann", "eng", KEY, at("09", "second try\n"));
    await openEditor();
    const note = screen.getByRole("note");
    expect(note).toHaveTextContent(
      "Texts from 2 earlier sessions are kept on this device. Newest first.",
    );
    const items = within(note).getAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent(formatInstant("2026-10-16T09:00:00Z"));
    expect(items[1]).toHaveTextContent(formatInstant("2026-10-16T08:00:00Z"));
    // The newer copy joined the history instead of replacing the offer.
    expect(
      readOffers("ann", "eng", OFFER).map((offer) => offer.content),
    ).toEqual(["second try\n", "first try\n"]);

    // Each names a line of its own text, so two close times are told apart.
    expect(items[0]).toHaveTextContent("second try");
    expect(items[1]).toHaveTextContent("first try");

    // The older one is chosen: it goes back, and only it leaves the list.
    await userEvent.click(
      within(items[1] as HTMLElement).getByRole("button", {
        name: `Put it back: ${formatInstant("2026-10-16T08:00:00Z")}, first try`,
      }),
    );
    expect(session.get().ytext?.toJSON()).toBe("first try\n");
    expect(
      readOffers("ann", "eng", OFFER).map((offer) => offer.content),
    ).toEqual(["second try\n"]);
    expect(screen.getByRole("note")).toHaveTextContent(
      "Text from an earlier session is kept on this device.",
    );
  });

  it("names an offer with no line to show by its time alone", async () => {
    serve("edit");
    live(room(PAGE));
    addOffer("ann", "eng", OFFER, at("08", "\n\n"));
    addOffer("ann", "eng", OFFER, at("09", "other\n"));
    await openEditor();
    expect(
      screen.getByRole("button", {
        name: `Put it back: ${formatInstant("2026-10-16T08:00:00Z")}`,
      }),
    ).toBeInTheDocument();
  });

  it("lists a text set aside in another tab before an older copy", async () => {
    serve("edit");
    live(room(PAGE));
    writeDraft("ann", "eng", KEY, at("07", "older copy\n"));
    addOffer("ann", "eng", OFFER, at("09", "newer offer\n"));
    await openEditor();
    const items = within(screen.getByRole("note")).getAllByRole("listitem");
    expect(items[0]).toHaveTextContent("newer offer");
    expect(items[1]).toHaveTextContent("older copy");
  });

  it("keeps the last three and drops them all on Discard all", async () => {
    serve("edit");
    live(room(PAGE));
    for (const [hour, text] of [
      ["06", "one\n"],
      ["07", "two\n"],
      ["08", "three\n"],
      ["09", "four\n"],
    ] as const) {
      addOffer("ann", "eng", OFFER, at(hour, text));
    }
    await openEditor();
    const note = screen.getByRole("note");
    expect(within(note).getAllByRole("listitem")).toHaveLength(3);
    expect(note).not.toHaveTextContent(formatInstant("2026-10-16T06:00:00Z"));
    await userEvent.click(
      within(note).getByRole("button", { name: "Discard all" }),
    );
    expect(screen.queryByRole("note")).toBeNull();
    expect(readOffers("ann", "eng", OFFER)).toEqual([]);
    expect(screen.getByRole("textbox", { name: "Page source" })).toHaveFocus();
  });

  it("leaves out an offer that only repeats the room", async () => {
    serve("edit");
    live(room(PAGE));
    addOffer("ann", "eng", OFFER, at("07", PAGE));
    addOffer("ann", "eng", OFFER, at("08", "other\n"));
    await openEditor();
    expect(
      screen.getByText("Text from an earlier session is kept on this device."),
    ).toBeVisible();
    expect(
      readOffers("ann", "eng", OFFER).map((offer) => offer.content),
    ).toEqual(["other\n"]);
  });
});

describe("the reflection editor after it has opened", () => {
  it("stays open when a status refetch fails, and says so beside it", async () => {
    let calls = 0;
    serve("edit", () => {
      calls += 1;
      if (calls > 1) {
        throw new ApiProblem(403, "forbidden", "no longer allowed");
      }
      return onStatus("edit");
    });
    live(room(PAGE));
    await openEditor();
    act(() => {
      focusManager.setFocused(false);
      focusManager.setFocused(true);
    });
    expect(await screen.findByText("no longer allowed")).toBeVisible();
    expect(screen.getByRole("textbox", { name: "Page source" })).toBeVisible();
  });

  it("stays open when the Reflection turns off, and shows the state beside it", async () => {
    let calls = 0;
    serve("edit", () => {
      calls += 1;
      return onStatus("edit", calls > 1 ? "off" : "on");
    });
    live(room(PAGE));
    await openEditor();
    act(() => {
      focusManager.setFocused(false);
      focusManager.setFocused(true);
    });
    await waitFor(() => {
      expect(calls).toBeGreaterThan(1);
    });
    expect(screen.getByRole("textbox", { name: "Page source" })).toBeVisible();
  });
});

describe("the reflection editor once the room has ended", () => {
  it("claims no save under a conflict or a pending save", async () => {
    for (const saveState of ["conflict", "pending"] as const) {
      serve("edit");
      const view = live({
        ...room(PAGE),
        saveState,
        closed: true,
        closedReason: "switched_off",
      });
      const { unmount } = renderApp("/d/eng/reflection-edit/guides/a.md");
      expect(
        await screen.findByText(
          "The session ended; your text is kept as a draft",
        ),
      ).toBeVisible();
      expect(screen.queryByText(/saved first/)).toBeNull();
      expect(readDraft("ann", "eng", KEY)?.content).toBe(
        (view.get().ytext as Y.Text).toJSON(),
      );
      unmount();
      localStorage.clear();
    }
  });

  it("keeps no draft when the room closed with everything saved", async () => {
    serve("edit");
    live({ ...room(PAGE), closed: true, closedReason: "internal" });
    await openEditor();
    expect(
      screen.getByText("The session ended. Your text was saved."),
    ).toBeVisible();
    expect(readDraft("ann", "eng", KEY)).toBeNull();
  });

  it("turns a lost connection into an ended, read-only session", async () => {
    serve("edit");
    const session = live(room(PAGE));
    const editor = await openEditor();
    type(session.get(), "typed\n");
    session.set({ saveState: "pending" });
    session.set({ status: "failed" });
    expect(
      screen.getByText("The session ended; your text is kept as a draft"),
    ).toBeVisible();
    expect(editor).toHaveAttribute("contenteditable", "false");
    expect(readDraft("ann", "eng", KEY)?.content).toBe(`${PAGE}typed\n`);
  });

  it("makes a closed room read-only and offers nothing to put back into it", async () => {
    serve("edit");
    writeDraft("ann", "eng", KEY, {
      content: "older\n",
      baseChecksum: "",
      savedAt: "",
    });
    live({ ...room(PAGE), closed: true, closedReason: "deleted" });
    const editor = await openEditor();
    expect(editor).toHaveAttribute("contenteditable", "false");
    expect(screen.queryByRole("button", { name: "Put it back" })).toBeNull();
  });
});

describe("the reflection editor when its page was moved", () => {
  it("says where the page went and opens it there with the typing carried", async () => {
    serve("edit");
    const session = live(room(PAGE));
    // The page at its new path holds what the move saved, without the line
    // typed after that save.
    const there = room(PAGE);
    const fallback = sessionMock.getMockImplementation();
    sessionMock.mockImplementation((options) => {
      const found = fallback?.(options) as CollabSession;
      return options.permalink === "reflection:guides/b.md" ? there : found;
    });
    await openEditor();
    type(session.get(), "typed after the last save\n");
    session.set({ saveState: "pending" });
    session.set({
      closed: true,
      closedReason: "moved",
      movedTo: { domain: "eng", path: "guides/b.md", permalink: null },
    });
    expect(
      screen.getByText("This page was moved to guides/b.md."),
    ).toBeVisible();
    expect(
      screen.queryByText("The session ended; your text is kept as a draft"),
    ).toBeNull();
    await userEvent.click(
      screen.getByRole("button", { name: "Open it there" }),
    );
    // Both keys followed the page, and the editor at the new path offers
    // the text because the page there does not hold it.
    expect(readDraft("ann", "eng", KEY)).toBeNull();
    expect(readDraft("ann", "eng", OFFER)).toBeNull();
    expect(
      await screen.findByText(
        "Text from an earlier session is kept on this device.",
      ),
    ).toBeVisible();
    expect(
      readDraft("ann", "eng", "reflection-offer:guides/b.md")?.content,
    ).toBe(`${PAGE}typed after the last save\n`);
    expect(screen.getByRole("button", { name: "Put it back" })).toBeVisible();
    expect(screen.queryByText(/This page was moved/)).toBeNull();
  });

  it.each([
    [
      "the page at its new path",
      { domain: "eng", path: "guides/b.md", permalink: null },
      "moved",
      "/d/eng/reflection/guides/b.md",
    ],
    ["the domain's Reflection", null, "moved", "/d/eng/reflection"],
    ["the domain's Reflection", null, "deleted", "/d/eng/reflection"],
  ] as const)(
    "sends Done to %s once the room ended (%s, %s)",
    async (_, movedTo, closedReason, href) => {
      serve("edit");
      live({ ...room(PAGE), closed: true, closedReason, movedTo });
      await openEditor();
      expect(screen.getByRole("link", { name: "Done" })).toHaveAttribute(
        "href",
        href,
      );
    },
  );

  it("keeps the text and names no place when the page moved where this person cannot go", async () => {
    serve("edit");
    live({
      ...room(PAGE),
      closed: true,
      closedReason: "moved",
      movedTo: null,
    });
    await openEditor();
    expect(
      screen.getByText(
        "This page was moved to a place you cannot open. Copy your text if you want to keep it.",
      ),
    ).toBeVisible();
    expect(screen.queryByRole("button", { name: "Open it there" })).toBeNull();
    expect(readDraft("ann", "eng", KEY)?.content).toBe(PAGE);
  });

  it("tells an editor that was still joining where the page went", async () => {
    serve("edit");
    sessionMock.mockReturnValue({
      ...soloCollabSession(),
      closed: true,
      closedReason: "moved",
      movedTo: { domain: "eng", path: "guides/b.md", permalink: null },
    });
    writeDraft("ann", "eng", KEY, {
      content: "kept from an earlier session\n",
      baseChecksum: "",
      savedAt: "",
    });
    renderApp("/d/eng/reflection-edit/guides/a.md");
    expect(
      await screen.findByText("This page was moved to guides/b.md."),
    ).toBeVisible();
    expect(
      screen.queryByText("Live editing is not available right now."),
    ).toBeNull();
    await userEvent.click(
      screen.getByRole("button", { name: "Open it there" }),
    );
    expect(readDraft("ann", "eng", KEY)).toBeNull();
    expect(readDraft("ann", "eng", "reflection:guides/b.md")?.content).toBe(
      "kept from an earlier session\n",
    );
  });

  it("names the new domain and carries the drafts into it after a rename", async () => {
    serve("edit");
    live({
      ...room(PAGE),
      closed: true,
      closedReason: "moved",
      movedTo: { domain: "ops", path: "guides/a.md", permalink: null },
    });
    writeDraft("ann", "eng", OFFER, {
      content: "set aside earlier\n",
      baseChecksum: "",
      savedAt: "",
    });
    await openEditor();
    expect(
      screen.getByText("This page was moved to ops / guides/a.md."),
    ).toBeVisible();
    await userEvent.click(
      screen.getByRole("button", { name: "Open it there" }),
    );
    expect(readDraft("ann", "eng", KEY)).toBeNull();
    expect(readDraft("ann", "eng", OFFER)).toBeNull();
    expect(readDraft("ann", "ops", KEY)?.content).toBe(PAGE);
    expect(readDraft("ann", "ops", OFFER)?.content).toBe("set aside earlier\n");
  });
});

describe("the reflection editor's kept text", () => {
  it("keeps a put-back text as a draft until the room saves it", async () => {
    serve("edit");
    const session = live(room(PAGE));
    writeDraft("ann", "eng", KEY, {
      content: "typed offline\n",
      baseChecksum: "",
      savedAt: "",
    });
    await openEditor();
    await userEvent.click(screen.getByRole("button", { name: "Put it back" }));
    expect(readDraft("ann", "eng", KEY)?.content).toBe("typed offline\n");
    expect(screen.getByRole("textbox", { name: "Page source" })).toHaveFocus();
    session.set({ saveState: "pending" });
    type(session.get(), "more\n");
    expect(readDraft("ann", "eng", KEY)?.content).toBe("typed offline\nmore\n");
    session.set({ saveState: "ok" });
    expect(readDraft("ann", "eng", KEY)).toBeNull();
  });

  it("leaves an offer intact in storage when a refusal comes", async () => {
    serve("edit");
    const session = live(room(PAGE));
    writeDraft("ann", "eng", KEY, {
      content: "older\n",
      baseChecksum: "",
      savedAt: "",
    });
    await openEditor();
    session.set({ saveState: "failed", saveDetail: REFUSAL });
    type(session.get(), "more\n");
    expect(readDraft("ann", "eng", OFFER)?.content).toBe("older\n");
    expect(readDraft("ann", "eng", KEY)?.content).toBe(`${PAGE}more\n`);
    expect(
      screen.getByText("Text from an earlier session is kept on this device."),
    ).toBeVisible();
  });

  it("discards only the offered text, never this tab's own copy", async () => {
    serve("edit");
    const session = live(room(PAGE));
    writeDraft("ann", "eng", KEY, {
      content: "older\n",
      baseChecksum: "",
      savedAt: "",
    });
    await openEditor();
    session.set({ saveState: "failed", saveDetail: REFUSAL });
    await userEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(readDraft("ann", "eng", KEY)?.content).toBe(PAGE);
    expect(readDraft("ann", "eng", OFFER)).toBeNull();
    expect(screen.getByRole("textbox", { name: "Page source" })).toHaveFocus();
  });

  it("leaves no draft behind that only repeats the room", async () => {
    serve("edit");
    live(room(PAGE));
    writeDraft("ann", "eng", KEY, {
      content: PAGE,
      baseChecksum: "",
      savedAt: "",
    });
    await openEditor();
    expect(readDraft("ann", "eng", KEY)).toBeNull();
  });
});

describe("the reflection editor's way in", () => {
  it("tells a reader with no right on the page that it is not theirs to open", async () => {
    serve("none");
    live(room(PAGE));
    renderApp("/d/eng/reflection-edit/guides/a.md");
    expect(await screen.findByText("You cannot edit this page.")).toBeVisible();
    expect(screen.queryByText(/can comment/)).toBeNull();
  });

  it("puts focus back on Try again when the room fails again", async () => {
    serve("edit");
    sessionMock.mockReturnValue(soloCollabSession());
    renderApp("/d/eng/reflection-edit/guides/a.md");
    await userEvent.click(
      await screen.findByRole("button", { name: "Try again" }),
    );
    expect(screen.getByRole("button", { name: "Try again" })).toHaveFocus();
  });

  it("puts focus in the editor when a retry joins the room", async () => {
    serve("edit");
    sessionMock.mockReturnValue(soloCollabSession());
    renderApp("/d/eng/reflection-edit/guides/a.md");
    const retry = await screen.findByRole("button", { name: "Try again" });
    sessionMock.mockReturnValue(room(PAGE));
    await userEvent.click(retry);
    expect(
      await screen.findByRole("textbox", { name: "Page source" }),
    ).toHaveFocus();
  });
});

/**
 * Files in the live editor: paste, drop and "Attach a file" run one upload
 * flow into the Reflection's own files, under a name nobody holds yet, and
 * the link lands in the room's text relative to the page.
 */
describe("the reflection editor takes files", () => {
  const ASSETS = "/domains/eng/reflection/assets";
  const SHOT_ROW = {
    path: "assets/2026/10/shot.png",
    mime: "image/png",
    size: 1,
    modified: "2026-10-01T00:00:00Z",
    sha256: "a",
  };

  /**
   * The editor's routes plus the Reflection's files: each listing call takes
   * the next answer (the last one repeats), each PUT the next verdict.
   */
  function serveFiles(
    listings: unknown[][],
    puts: ((path: string) => unknown)[] = [],
  ) {
    serve("edit");
    const base = apiMock.getMockImplementation();
    const lists = [...listings];
    const verdicts = [...puts];
    apiMock.mockImplementation(async (path: string, init?: RequestInit) => {
      if (path === ASSETS) {
        const rows = lists.length > 1 ? lists.shift() : lists[0];
        return { assets: rows ?? [] };
      }
      if (path.startsWith(`${ASSETS}/`) && init?.method === "PUT") {
        const stored = `assets/${path.slice(ASSETS.length + 1)}`;
        const verdict = verdicts.shift();
        return verdict === undefined
          ? { path: stored, mime: "image/png", size: 8, sha256: "b" }
          : await verdict(stored);
      }
      return await base?.(path, init);
    });
  }

  function puts(): string[] {
    return apiMock.mock.calls
      .filter(([, init]) => init?.method === "PUT")
      .map(([path]) => path);
  }

  function carrying(type: "paste" | "drop", files: File[]): Event {
    const event = new Event(type, { bubbles: true, cancelable: true });
    Object.defineProperty(
      event,
      type === "paste" ? "clipboardData" : "dataTransfer",
      { value: { files, types: ["Files"], getData: () => "" } },
    );
    return event;
  }

  function png(name = "shot.png"): File {
    return new File(["x"], name, { type: "image/png" });
  }

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 9, 15, 12));
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("reports an upload that fails after its screen has gone, naming the file and the page", async () => {
    let fail: (cause: unknown) => void = () => undefined;
    serveFiles(
      [[]],
      [
        () =>
          new Promise((_resolve, reject) => {
            fail = reject;
          }),
      ],
    );
    live(room(PAGE));
    const editor = await openEditor();
    editor.dispatchEvent(carrying("paste", [png("flow.png")]));
    await waitFor(() => {
      expect(puts()).toHaveLength(1);
    });
    // The person leaves while the bytes are on their way.
    await userEvent.click(screen.getByRole("link", { name: "Done" }));
    await waitFor(() => {
      expect(
        screen.queryByRole("textbox", { name: "Page source" }),
      ).not.toBeInTheDocument();
    });
    act(() => {
      fail(new ApiProblem(507, "Insufficient Storage", "the disk is full"));
    });
    expect(
      await screen.findByText("The upload of flow.png to A did not finish."),
    ).toBeInTheDocument();
    expect(screen.getByText("the disk is full")).toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "Dismiss: flow.png" }),
    );
    expect(
      screen.queryByText("The upload of flow.png to A did not finish."),
    ).toBeNull();
  });

  it("says an upload that fails across a reconnect in the editor, not as a notice", async () => {
    let fail: (cause: unknown) => void = () => undefined;
    serveFiles(
      [[]],
      [
        () =>
          new Promise((_resolve, reject) => {
            fail = reject;
          }),
      ],
    );
    const session = live(room(PAGE));
    const editor = await openEditor();
    editor.dispatchEvent(carrying("paste", [png("flow.png")]));
    await waitFor(() => {
      expect(puts()).toHaveLength(1);
    });
    // The room reconnects: a new buffer under the same editor.
    session.set({ epoch: "e2" });
    act(() => {
      fail(new ApiProblem(507, "Insufficient Storage", "the disk is full"));
    });
    expect(await screen.findByText("the disk is full")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Upload notices" })).toBeNull();
    expect(screen.getByRole("textbox", { name: "Page source" })).toBeVisible();
  });

  it("reports a file stored after its screen has gone as uploaded without its link", async () => {
    let land: (value: unknown) => void = () => undefined;
    serveFiles(
      [[]],
      [
        (stored) =>
          new Promise((resolve) => {
            land = () => {
              resolve({
                path: stored,
                mime: "image/png",
                size: 8,
                sha256: "b",
              });
            };
          }),
      ],
    );
    live(room(PAGE));
    const editor = await openEditor();
    editor.dispatchEvent(carrying("paste", [png("flow.png")]));
    await waitFor(() => {
      expect(puts()).toHaveLength(1);
    });
    await userEvent.click(screen.getByRole("link", { name: "Done" }));
    await waitFor(() => {
      expect(
        screen.queryByRole("textbox", { name: "Page source" }),
      ).not.toBeInTheDocument();
    });
    act(() => {
      land(undefined);
    });
    expect(
      await screen.findByText(
        "flow.png was uploaded to A, but the link was not added.",
      ),
    ).toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "Dismiss: flow.png" }),
    );
  });

  it("a contributor's paste uploads under a free name and inserts a relative link", async () => {
    serveFiles([[SHOT_ROW]]);
    const session = live(room(PAGE));
    const editor = await openEditor();
    editor.dispatchEvent(carrying("paste", [png()]));
    await waitFor(() => {
      expect(session.get().ytext?.toJSON()).toContain(
        "![shot.png](../assets/2026/10/shot-2.png)",
      );
    });
    expect(puts()).toEqual([
      "/domains/eng/reflection/assets/2026/10/shot-2.png",
    ]);
  });

  it("a file dropped on a heading line lands below its id line", async () => {
    serveFiles([[]]);
    const session = live(room(PAGE));
    const editor = await openEditor();
    const view = EditorView.findFromDOM(editor);
    act(() => {
      view?.dispatch({ selection: { anchor: PAGE.indexOf("## One") + 6 } });
    });
    const event = carrying("drop", [png()]);
    editor.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    await waitFor(() => {
      expect(session.get().ytext?.toJSON()).toContain(
        "## One\n<!-- r:aaaa1111 -->\n\n![shot.png](../assets/2026/10/shot.png)\n",
      );
    });
  });

  it("the picker attaches through the same flow", async () => {
    serveFiles([[]]);
    const session = live(room(PAGE));
    await openEditor();
    const button = screen.getByRole("button", { name: "Attach a file" });
    expect(button).toBeEnabled();
    const input =
      document.querySelector<HTMLInputElement>('input[type="file"]');
    // Once: nothing opens a real dialog in jsdom, and the upload below clicks
    // the input too.
    input?.addEventListener(
      "click",
      (event) => {
        event.preventDefault();
      },
      { once: true },
    );
    await userEvent.click(button);
    await userEvent.upload(input as HTMLInputElement, png());
    await waitFor(() => {
      expect(session.get().ytext?.toJSON()).toContain(
        "![shot.png](../assets/2026/10/shot.png)",
      );
    });
  });

  /** Pick `file` through "Attach a file", as the toolbar's picker hands it on. */
  async function pick(file: File): Promise<void> {
    const input =
      document.querySelector<HTMLInputElement>('input[type="file"]');
    input?.addEventListener(
      "click",
      (event) => {
        event.preventDefault();
      },
      { once: true },
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Attach a file" }),
    );
    await userEvent.upload(input as HTMLInputElement, file);
  }

  it("the picker puts the file at the end when no caret was placed", async () => {
    serveFiles([[]]);
    const session = live(room(PAGE));
    await openEditor();
    await pick(png());
    // Not above the title, where the untouched caret sits: at the end, on a
    // line of its own.
    await waitFor(() => {
      expect(session.get().ytext?.toJSON()).toBe(
        `${PAGE}\n![shot.png](../assets/2026/10/shot.png)\n`,
      );
    });
  });

  it("the picker puts the file below the line where the caret was placed", async () => {
    serveFiles([[]]);
    const session = live(room(PAGE));
    const editor = await openEditor();
    const view = EditorView.findFromDOM(editor);
    act(() => {
      view?.dispatch({
        selection: { anchor: "# A".length },
        userEvent: "select.pointer",
      });
    });
    await pick(png());
    await waitFor(() => {
      expect(session.get().ytext?.toJSON()).toBe(
        PAGE.replace(
          "# A\n",
          "# A\n\n![shot.png](../assets/2026/10/shot.png)\n\n",
        ),
      );
    });
  });

  it("a name taken between listing and upload is tried once more under the next free name", async () => {
    serveFiles(
      [[], [SHOT_ROW]],
      [
        () => {
          throw new ApiProblem(403, "forbidden", "a file is already there");
        },
      ],
    );
    const session = live(room(PAGE));
    const editor = await openEditor();
    editor.dispatchEvent(carrying("paste", [png()]));
    await waitFor(() => {
      expect(session.get().ytext?.toJSON()).toContain(
        "![shot.png](../assets/2026/10/shot-2.png)",
      );
    });
    expect(puts()).toEqual([
      "/domains/eng/reflection/assets/2026/10/shot.png",
      "/domains/eng/reflection/assets/2026/10/shot-2.png",
    ]);
    expect(screen.queryByText("a file is already there")).toBeNull();
  });

  it("the server's refusal is shown as it was sent, and is no failed save", async () => {
    const detail = "the attachment is larger than 10 MiB";
    serveFiles(
      [[]],
      [
        () => {
          throw new ApiProblem(413, "too large", detail);
        },
      ],
    );
    const session = live(room(PAGE));
    const editor = await openEditor();
    editor.dispatchEvent(carrying("paste", [png()]));
    expect(await screen.findByRole("alert")).toHaveTextContent(detail);
    expect(session.get().ytext?.toJSON()).toBe(PAGE);
    expect(readDraft("ann", "eng", KEY)).toBeNull();
    expect(readDraft("ann", "eng", OFFER)).toBeNull();
    expect(screen.queryByText(/kept as a draft/)).toBeNull();
    // A later batch clears the line.
    editor.dispatchEvent(carrying("paste", [png()]));
    await waitFor(() => {
      expect(screen.queryByRole("alert")).toBeNull();
    });
  });

  it("a file off the allowlist is refused before any upload", async () => {
    serveFiles([[]]);
    live(room(PAGE));
    const editor = await openEditor();
    editor.dispatchEvent(
      carrying("paste", [new File(["x"], "tool.exe", { type: "" })]),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "tool.exe is not a kind of file Crystalline stores.",
    );
    expect(puts()).toEqual([]);
  });

  it("refreshes the Reflection's file list after a batch", async () => {
    const invalidate = vi.spyOn(QueryClient.prototype, "invalidateQueries");
    serveFiles([[]]);
    const session = live(room(PAGE));
    const editor = await openEditor();
    editor.dispatchEvent(carrying("paste", [png()]));
    await waitFor(() => {
      expect(session.get().ytext?.toJSON()).toContain("shot.png");
    });
    await waitFor(() => {
      expect(invalidate).toHaveBeenCalledWith({
        queryKey: ["reflection", "eng", "assets"],
      });
    });
  });

  it("turns the button off while reconnecting, and a paste still uploads", async () => {
    serveFiles([[]]);
    const session = live(room(PAGE));
    const editor = await openEditor();
    session.set({ status: "reconnecting" });
    expect(
      screen.getByRole("button", { name: "Attach a file" }),
    ).toBeDisabled();
    editor.dispatchEvent(carrying("paste", [png()]));
    await waitFor(() => {
      expect(session.get().ytext?.toJSON()).toContain(
        "![shot.png](../assets/2026/10/shot.png)",
      );
    });
  });

  it("takes no file into a room that has ended", async () => {
    serveFiles([[]]);
    const session = live(room(PAGE));
    const editor = await openEditor();
    session.set({ closed: true, closedReason: "left", saveState: "ok" });
    expect(
      screen.getByRole("button", { name: "Attach a file" }),
    ).toBeDisabled();
    // The whole bar is off with it: the text can take no edit now.
    expect(screen.getByRole("button", { name: "Bold" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Heading" })).toBeDisabled();
    editor.dispatchEvent(carrying("paste", [png()]));
    const drop = carrying("drop", [png()]);
    editor.dispatchEvent(drop);
    expect(drop.defaultPrevented).toBe(true);
    await act(async () => {
      await Promise.resolve();
    });
    expect(
      apiMock.mock.calls.filter(([path]) => path.startsWith(ASSETS)),
    ).toEqual([]);
    expect(session.get().ytext?.toJSON()).toBe(PAGE);
  });

  it("draws a Reflection file's image under its link, and no engram file", async () => {
    serveFiles([[]]);
    live(
      room(`${PAGE}\n![a](../assets/2026/10/a.png)\n\n![x](assets/x.png)\n`),
    );
    const editor = await openEditor();
    const root = editor.closest(".cm-editor") as HTMLElement;
    await waitFor(() => {
      expect(
        [...root.querySelectorAll(".cm-image-preview img")].map((img) =>
          img.getAttribute("src"),
        ),
      ).toEqual(["/api/v1/domains/eng/reflection/assets/2026/10/a.png"]);
    });
  });

  it("draws a preview from the listing's version and follows a new one, never writing it", async () => {
    const A_ROW = {
      path: "assets/2026/10/a.png",
      mime: "image/png",
      size: 1,
      modified: "2026-10-01T00:00:00Z",
      sha256: "1111111111111111",
    };
    serveFiles([[A_ROW], [{ ...A_ROW, sha256: "2222222222222222" }]]);
    const session = live(room(`${PAGE}\n![a](../assets/2026/10/a.png)\n`));
    const editor = await openEditor();
    const root = editor.closest(".cm-editor") as HTMLElement;
    const srcs = () =>
      [...root.querySelectorAll(".cm-image-preview img")].map((img) =>
        img.getAttribute("src"),
      );
    // The listing that answered first is the baseline: the bytes on screen
    // are the ones the plain address serves, so no version is added.
    await waitFor(() => {
      expect(apiMock.mock.calls.some(([path]) => path === ASSETS)).toBe(true);
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(srcs()).toEqual([
      "/api/v1/domains/eng/reflection/assets/2026/10/a.png",
    ]);
    // An upload refreshes the listing; the preview follows its new version.
    editor.dispatchEvent(carrying("paste", [png()]));
    await waitFor(() => {
      expect(srcs()).toContain(
        "/api/v1/domains/eng/reflection/assets/2026/10/a.png?v=222222222222",
      );
    });
    expect(session.get().ytext?.toJSON()).not.toContain("?v=");
  });

  it("reads the listing once the room's text links a file, and not before", async () => {
    serveFiles([[SHOT_ROW]]);
    const session = live(room(PAGE));
    await openEditor();
    const listings = () =>
      apiMock.mock.calls.filter(([path]) => path === ASSETS).length;
    await act(async () => {
      await Promise.resolve();
    });
    expect(listings()).toBe(0);
    // Somebody else in the room links a file.
    type(session.get(), "\n![shot](../assets/2026/10/shot.png)\n");
    await waitFor(() => {
      expect(listings()).toBe(1);
    });
  });

  it("a file replaced on the reading page is drawn anew in the editor", async () => {
    const FLOW = {
      path: "assets/flow.png",
      mime: "image/png",
      size: 2048,
      modified: "2026-10-01T00:00:00Z",
      sha256: "1111111111111111",
    };
    const TEXT = "# A\n\n![Flow](../assets/flow.png)\n";
    let sha = FLOW.sha256;
    serve("curate");
    const base = apiMock.getMockImplementation();
    apiMock.mockImplementation(async (path: string, init?: RequestInit) => {
      if (path === ASSETS) {
        return { assets: [{ ...FLOW, sha256: sha }] };
      }
      if (path === `${ASSETS}/flow.png` && init?.method === "PUT") {
        sha = "2222222222222222";
        return { path: FLOW.path, mime: "image/png", size: 3, sha256: sha };
      }
      if (path === "/domains/eng/reflections/guides/a.md") {
        return {
          domain: "eng",
          path: "guides/a.md",
          title: "A",
          markdown: TEXT,
          checksum: "c1",
          sections: [],
          threads: [],
        };
      }
      return await base?.(path, init);
    });
    live(room(TEXT));
    renderApp("/d/eng/reflection/guides/a.md");
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Replace flow.png" }),
    );
    await user.upload(
      screen.getByLabelText("Replace flow.png: pick a file"),
      new File(["new"], "flow.png", { type: "image/png" }),
    );
    await user.click(screen.getByRole("button", { name: "Replace" }));
    await waitFor(() => {
      expect(
        screen.getByRole("img", { name: "Flow" }).getAttribute("src"),
      ).toMatch(/\?v=222222222222$/);
    });
    await user.click(screen.getByRole("link", { name: "Edit" }));
    const editor = await screen.findByRole("textbox", { name: "Page source" });
    const root = editor.closest(".cm-editor") as HTMLElement;
    await waitFor(() => {
      expect(
        [...root.querySelectorAll(".cm-image-preview img")].map((img) =>
          img.getAttribute("src"),
        ),
      ).toEqual([
        "/api/v1/domains/eng/reflection/assets/flow.png?v=222222222222",
      ]);
    });
  });

  it("adds no link when the room ends while the file is on its way", async () => {
    let land: (value: unknown) => void = () => undefined;
    serveFiles(
      [[]],
      [
        (stored) =>
          new Promise((resolve) => {
            land = () => {
              resolve({
                path: stored,
                mime: "image/png",
                size: 8,
                sha256: "b",
              });
            };
          }),
      ],
    );
    const session = live(room(PAGE));
    const editor = await openEditor();
    editor.dispatchEvent(carrying("paste", [png()]));
    await waitFor(() => {
      expect(puts()).toHaveLength(1);
    });
    session.set({ closed: true, closedReason: "left", saveState: "ok" });
    await act(async () => {
      land(undefined);
      await Promise.resolve();
    });
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Uploaded to assets/2026/10/shot.png. The session ended, so no link was added. Add it when you edit the page again.",
    );
    expect(session.get().ytext?.toJSON()).toBe(PAGE);
    for (const key of [KEY, OFFER]) {
      expect(readDraft("ann", "eng", key)?.content ?? "").not.toContain(
        "shot.png",
      );
    }
  });

  it("takes no file into a room whose connection failed for good", async () => {
    serveFiles([[]]);
    const session = live(room(PAGE));
    const editor = await openEditor();
    session.set({ status: "failed" });
    expect(
      screen.getByRole("button", { name: "Attach a file" }),
    ).toBeDisabled();
    editor.dispatchEvent(carrying("paste", [png()]));
    await act(async () => {
      await Promise.resolve();
    });
    expect(
      apiMock.mock.calls.filter(([path]) => path.startsWith(ASSETS)),
    ).toEqual([]);
  });
});

describe("the reflection editor's kept text notice", () => {
  const KEPT_PAGE = PAGE.replace("First.", "First, as the room kept it.");

  function keptEntry(overrides: Record<string, unknown> = {}) {
    return {
      id: "k1",
      draft: false,
      kind: "reflection",
      path: "guides/a.md",
      kept_at: "2026-10-15T08:00:00Z",
      bytes: KEPT_PAGE.length,
      sender: null,
      ...overrides,
    };
  }

  /** `serve`'s routes plus the kept listing; records every drop. */
  function serveKept(list: () => unknown) {
    const dropped: string[] = [];
    let gone = false;
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () =>
          meResponse({ user: userFixture({ name: "ann", role: "viewer" }) }),
        "/domains": () => ({ behavior: [], domains: [] }),
        "/reflection/domains": () => ({ domains: [] }),
        "/domains/eng/reflection": () => onStatus("edit"),
        "/domains/eng/kept": () =>
          gone ? { domain: "eng", kept: [] } : list(),
        "/domains/eng/kept/k1": (_path, init) => {
          if (init?.method === "DELETE") {
            dropped.push("k1");
            gone = true;
            return undefined;
          }
          return { domain: "eng", ...keptEntry(), text: KEPT_PAGE };
        },
      }),
    );
    return dropped;
  }

  it("offers the server's kept text beside this device's own copy, in words that tell them apart", async () => {
    serveKept(() => ({ domain: "eng", kept: [keptEntry()] }));
    sessionMock.mockReturnValue(room(PAGE));
    writeDraft("ann", "eng", "reflection:guides/a.md", {
      content: "typed offline\n",
      baseChecksum: "c0",
      savedAt: "2026-09-30T10:00:00Z",
    });
    await openEditor();
    expect(
      await screen.findByText("The server kept unsaved text for this page."),
    ).toBeVisible();
    expect(
      screen.getByText("Text from an earlier session is kept on this device."),
    ).toBeVisible();
    const notice = screen.getByRole("note", { name: "Kept text" });
    await userEvent.click(within(notice).getByRole("button", { name: "Show" }));
    const dialog = await screen.findByRole("dialog", { name: "Kept text" });
    expect(
      await within(dialog).findByLabelText("The kept text"),
    ).toHaveTextContent("First, as the room kept it.");
  });

  it("Replace with it changes the room's text as one undoable change, and never drops the kept copy on its own", async () => {
    const dropped = serveKept(() => ({ domain: "eng", kept: [keptEntry()] }));
    const session = live(room(PAGE));
    await openEditor();
    const notice = await screen.findByRole("note", { name: "Kept text" });
    await userEvent.click(
      within(notice).getByRole("button", { name: "Replace with it" }),
    );
    const ytext = session.get().ytext;
    await waitFor(() => {
      expect(ytext?.toJSON()).toBe(KEPT_PAGE);
    });
    expect(
      await within(notice).findByText(
        "Your text is back in the editor. Drop the kept copy once it is saved.",
      ),
    ).toBeVisible();
    session.set({ saveState: "pending" });
    session.set({ saveState: "ok" });
    expect(dropped).toEqual([]);
    // Drop is the person's, and works.
    await userEvent.click(
      within(notice).getByRole("button", { name: "Drop the kept text" }),
    );
    await userEvent.click(
      within(notice).getByRole("button", { name: "Drop it" }),
    );
    await waitFor(() => {
      expect(dropped).toEqual(["k1"]);
    });
    // The room's own undo takes the whole change back.
    const host = screen
      .getByRole("textbox", { name: "Page source" })
      .closest(".cm-editor");
    const view = EditorView.findFromDOM(host as HTMLElement);
    expect(view).not.toBeNull();
    const undo = yUndoManagerKeymap.find((binding) => binding.key === "Mod-z");
    act(() => {
      undo?.run?.(view as EditorView);
    });
    expect(ytext?.toJSON()).toBe(PAGE);
  });

  it("offers no replace while the room is not live", async () => {
    serveKept(() => ({ domain: "eng", kept: [keptEntry()] }));
    const session = live(room(PAGE));
    await openEditor();
    const notice = await screen.findByRole("note", { name: "Kept text" });
    session.set({ live: false });
    expect(
      within(notice).queryByRole("button", { name: "Replace with it" }),
    ).toBeNull();
  });

  it("offers no replace once the room has ended", async () => {
    serveKept(() => ({ domain: "eng", kept: [keptEntry()] }));
    const session = live(room(PAGE));
    await openEditor();
    const notice = await screen.findByRole("note", { name: "Kept text" });
    expect(
      within(notice).getByRole("button", { name: "Replace with it" }),
    ).toBeVisible();
    session.set({ closed: true, closedReason: "off" });
    expect(
      within(notice).queryByRole("button", { name: "Replace with it" }),
    ).toBeNull();
  });

  it("Drop asks first, then drops the kept text", async () => {
    const dropped = serveKept(() => ({ domain: "eng", kept: [keptEntry()] }));
    const session = room(PAGE);
    sessionMock.mockReturnValue(session);
    await openEditor();
    const notice = await screen.findByRole("note", { name: "Kept text" });
    await userEvent.click(
      within(notice).getByRole("button", { name: "Drop the kept text" }),
    );
    expect(dropped).toEqual([]);
    await userEvent.click(
      within(notice).getByRole("button", { name: "Drop it" }),
    );
    await waitFor(() => {
      expect(dropped).toEqual(["k1"]);
    });
    expect(session.ytext?.toJSON()).toBe(PAGE);
  });

  it("is not there for another page, a comment, an engram or a draft", async () => {
    serveKept(() => ({
      domain: "eng",
      kept: [
        keptEntry({ path: "guides/b.md" }),
        keptEntry({ id: "k2", kind: "comment", sender: "ben" }),
        keptEntry({ id: "k3", kind: "engram" }),
        keptEntry({ id: "k4", draft: true }),
      ],
    }));
    sessionMock.mockReturnValue(room(PAGE));
    await openEditor();
    await waitFor(() => {
      expect(
        apiMock.mock.calls.some(([path]) => path === "/domains/eng/kept"),
      ).toBe(true);
    });
    expect(screen.queryByRole("note", { name: "Kept text" })).toBeNull();
  });

  it("is not there, and says nothing, without a right to kept texts", async () => {
    serveKept(() => {
      throw new ApiProblem(403, "forbidden", "kept texts need write access");
    });
    sessionMock.mockReturnValue(room(PAGE));
    await openEditor();
    await waitFor(() => {
      expect(
        apiMock.mock.calls.some(([path]) => path === "/domains/eng/kept"),
      ).toBe(true);
    });
    expect(screen.queryByRole("note", { name: "Kept text" })).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
