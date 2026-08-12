/**
 * The MANIFEST editor's own session behavior, as opposed to the route wiring
 * and the admin gate `ManifestPage.test.tsx` proves.
 *
 * The screen holds none of this itself any more: buffer, checksum, the Mod-S
 * save and the draft safety net all come from `useEditorSession`, which the
 * engram editor uses too. That is exactly why the screen needs its own net -
 * a change to the shared shell must not be able to quietly unwire the editor
 * that has no other test watching it, and the save keybinding in particular
 * now raises the shared `crystalline:save` event rather than a name of its
 * own.
 */

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import type { Answer } from "../test/harness";
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

const MARKDOWN = "---\ntitle: Eng\n---\n\n# Eng\n\nRoute here for eng.\n";

/** The detail read the editor mounts on, in the engine's own shape. */
function manifestResponse(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    markdown: MARKDOWN,
    checksum: "m1",
    ...overrides,
  };
}

/** An empty tree, so the sidebar under the editor mounts with nothing else
 *  to say. */
function emptyTree() {
  return { domain: "eng", path: "", folders: [], engrams: [] };
}

/** The app signed in as the admin this screen is gated to. */
function serveEditor(routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role: "admin" }) }),
      "/domains": domainsResponse,
      "/domains/eng/manifest": () => manifestResponse(),
      "/domains/eng/tree": () => emptyTree(),
      "/validate": () => ({ findings: [], errors: 0 }),
      ...routes,
    }),
  );
}

/** A manifest route that answers a PUT from `put` and a GET from the fixture. */
function savingManifest(put: () => unknown): Record<string, Answer> {
  return {
    "/domains/eng/manifest": (_path, init) =>
      init?.method === "PUT" ? put() : manifestResponse(),
  };
}

/** The `If-Match` header of the first PUT the mock has seen. */
function firstIfMatch(): string | undefined {
  const call = apiMock.mock.calls.find(([, init]) => init?.method === "PUT");
  const headers = call?.[1]?.headers as Record<string, string> | undefined;
  return headers?.["If-Match"];
}

beforeEach(() => {
  apiMock.mockReset();
  localStorage.clear();
});

describe("the MANIFEST editor", () => {
  it("loads the exact file text and saves it back with the If-Match token", async () => {
    const put = vi.fn(() => manifestResponse({ checksum: "m2" }));
    serveEditor(savingManifest(put));
    renderApp("/d/eng/manifest/edit");
    const editor = await screen.findByLabelText("MANIFEST source");
    await waitFor(() => {
      expect(editor.textContent).toContain("Route here for eng.");
    });
    // Frontmatter is in the buffer too: the whole file is the document.
    expect(editor.textContent).toContain("title: Eng");

    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(put).toHaveBeenCalled();
    });
    expect(firstIfMatch()).toBe('"m1"');
    expect(await screen.findByText("Saved")).toBeInTheDocument();
  });

  it("carries the format bar's table verbs", async () => {
    // A MANIFEST is markdown like any engram, and this screen builds its own
    // extension array: the context listener has to be in THIS one too, or the
    // segment never appears here however well the engram editor works.
    serveEditor();
    renderApp("/d/eng/manifest/edit");
    const editor = await screen.findByLabelText("MANIFEST source");
    await waitFor(() => {
      expect(editor.textContent).toContain("Route here for eng.");
    });
    expect(
      screen.queryByRole("button", { name: "Add column after" }),
    ).toBeNull();

    // Inserting a table leaves the caret in the header row it just wrote.
    await userEvent.click(screen.getByRole("button", { name: "Insert table" }));
    expect(
      await screen.findByRole("button", { name: "Add column after" }),
    ).toBeInTheDocument();
  });

  it("saves from inside the buffer, on the keyboard", async () => {
    const put = vi.fn(() => manifestResponse({ checksum: "m2" }));
    serveEditor(savingManifest(put));
    renderApp("/d/eng/manifest/edit");
    const editor = await screen.findByLabelText("MANIFEST source");
    editor.focus();
    // Mod is Control off the Mac, which is what the test environment says it
    // is running on. The keymap raises the shared save event on the editor's
    // own node and the session answers it there.
    await userEvent.keyboard("{Control>}s{/Control}");
    await waitFor(() => {
      expect(put).toHaveBeenCalled();
    });
    expect(firstIfMatch()).toBe('"m1"');
  });

  it("offers a stored draft and restores it into the buffer", async () => {
    // The MANIFEST has no permalink of its own; the draft key uses the fixed
    // MANIFEST slot in its place.
    localStorage.setItem(
      "fluid.draft.ada.eng/MANIFEST",
      JSON.stringify({
        content: MARKDOWN.replace("Route here for eng.", "A recovered line."),
        baseChecksum: "m1",
        savedAt: "2026-08-09T10:00:00Z",
      }),
    );
    serveEditor();
    renderApp("/d/eng/manifest/edit");
    expect(await screen.findByText(/unsaved draft/i)).toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "Restore draft" }),
    );
    await waitFor(() => {
      expect(screen.getByLabelText("MANIFEST source").textContent).toContain(
        "A recovered line.",
      );
    });
  });

  it("discarding the draft keeps the server text and drops the banner", async () => {
    localStorage.setItem(
      "fluid.draft.ada.eng/MANIFEST",
      JSON.stringify({
        content: "something else entirely",
        baseChecksum: "m1",
        savedAt: "",
      }),
    );
    serveEditor();
    renderApp("/d/eng/manifest/edit");
    await screen.findByText(/unsaved draft/i);
    await userEvent.click(
      screen.getByRole("button", { name: "Discard draft" }),
    );
    await waitFor(() => {
      expect(screen.queryByText(/unsaved draft/i)).not.toBeInTheDocument();
    });
    expect(localStorage.getItem("fluid.draft.ada.eng/MANIFEST")).toBeNull();
    expect(screen.getByLabelText("MANIFEST source").textContent).toContain(
      "Route here for eng.",
    );
  });
});
