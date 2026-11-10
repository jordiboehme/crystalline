/**
 * The kept text note on its own: which words it says, when it offers to
 * replace the editor's text, that the server copy goes only once a save holds
 * the text, and every way that can fail without losing the copy. The editor
 * tests next door cover the wiring into both surfaces.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { KeptTextEntry } from "../api/kept";
import type { AuthValue, Capabilities } from "../auth/AuthContext";
import { AuthContext } from "../auth/AuthContext";
import { userFixture } from "../test/harness";
import { KeptTextNotice } from "./KeptTextNotice";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn() };
});
const apiMock = vi.mocked(api);

const KEPT = "# A\n\nThe room's text.\n";

function entry(over: Partial<KeptTextEntry> = {}): KeptTextEntry {
  return {
    id: "k1",
    draft: false,
    kind: "engram",
    path: "a.md",
    file: false,
    kept_at: "2026-10-15T08:00:00Z",
    bytes: KEPT.length,
    sender: null,
    ...over,
  };
}

function auth(readOnly = false): AuthValue {
  const capabilities: Capabilities = {
    anonymous: false,
    readOnly,
    role: "editor",
    canWrite: !readOnly,
    canAdminister: false,
    canShare: false,
    needsSetup: false,
    oauth: false,
    serverVersion: "test",
  };
  return {
    user: userFixture(),
    capabilities,
    login: vi.fn(),
    setup: vi.fn(),
    logout: vi.fn(),
  };
}

interface Server {
  dropped: string[];
  /** Answers the next read of the text; the default answers at once. */
  read: () => Promise<unknown>;
  /** Answers the next delete; the default deletes. */
  remove: () => Promise<unknown>;
}

/** The kept routes, listing `entries` until a delete takes one away. */
function serve(entries: KeptTextEntry[]): Server {
  let listed = entries;
  const server: Server = {
    dropped: [],
    read: () => Promise.resolve({ domain: "eng", ...entry(), text: KEPT }),
    remove: () => Promise.resolve(undefined),
  };
  apiMock.mockImplementation(async (path: string, init?: RequestInit) => {
    if (path === "/domains/eng/kept") {
      return { domain: "eng", kept: listed };
    }
    if (path === "/domains/eng/kept/k1") {
      if (init?.method === "DELETE") {
        const answer = (await server.remove()) as never;
        server.dropped.push("k1");
        listed = listed.filter((row) => row.id !== "k1");
        return answer;
      }
      return await server.read();
    }
    throw new ApiProblem(404, "not found", `no stub for ${path}`);
  });
  return server;
}

type Props = Partial<Parameters<typeof KeptTextNotice>[0]>;

/**
 * The editor's saves as the note sees them: each save gets a number when it
 * starts, and the newest that landed is remembered, as the solo session
 * counts them.
 */
const saves = { started: 0, landed: 0 };

function startSave(): number {
  saves.started += 1;
  return saves.started;
}

function mount(props: Props = {}, readOnly = false) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const value = auth(readOnly);
  const view = (next: Props): ReactElement => (
    <QueryClientProvider client={client}>
      <AuthContext value={value}>
        <KeptTextNotice
          domain="eng"
          kind="engram"
          path="a.md"
          draft={false}
          ready
          onReplace={() => true}
          soloSaves={{
            started: () => saves.started,
            landedAfter: (count) => saves.landed > count,
          }}
          {...next}
        />
      </AuthContext>
    </QueryClientProvider>
  );
  const rendered = render(view(props));
  return {
    rerender: (next: Props) => {
      rendered.rerender(view({ ...props, ...next }));
    },
  };
}

function note(): HTMLElement {
  return screen.getByRole("note", { name: "Kept text" });
}

beforeEach(() => {
  apiMock.mockReset();
  saves.started = 0;
  saves.landed = 0;
});

describe("the kept text note", () => {
  it("says what was kept, and from whom when the server knows", async () => {
    serve([entry()]);
    mount();
    expect(
      await screen.findByText("The server kept unsaved text for this page."),
    ).toBeVisible();
  });

  it("names the sender", async () => {
    serve([entry({ sender: "ben" })]);
    mount();
    expect(
      await screen.findByText(
        "The server kept unsaved text for this page. From ben.",
      ),
    ).toBeVisible();
  });

  it("says when it is the reader's own draft", async () => {
    serve([entry({ draft: true })]);
    mount({ draft: true });
    expect(
      await screen.findByText("The server kept unsaved text from your draft."),
    ).toBeVisible();
  });

  it("offers nothing while it is not known whose document this is", async () => {
    serve([entry()]);
    mount({ draft: null });
    await waitFor(() => {
      expect(apiMock).toHaveBeenCalledWith("/domains/eng/kept");
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.queryByRole("note")).toBeNull();
  });

  it("Replace with it puts the text in and keeps the server copy until a later save lands", async () => {
    const server = serve([entry()]);
    const onReplace = vi.fn(() => true);
    const { rerender } = mount({ onReplace });
    await userEvent.click(
      await screen.findByRole("button", { name: "Replace with it" }),
    );
    expect(onReplace).toHaveBeenCalledWith(KEPT);
    expect(await within(note()).findByRole("status")).toHaveTextContent(
      "Waiting for the save.",
    );
    expect(
      within(note()).queryByRole("button", { name: "Replace with it" }),
    ).toBeNull();
    expect(server.dropped).toEqual([]);
    // The person edits on top of it and saves: whatever the text is now,
    // the save began after the replace, so the kept copy goes.
    const save = startSave();
    saves.landed = save;
    rerender({ onReplace });
    await waitFor(() => {
      expect(server.dropped).toEqual(["k1"]);
    });
    await waitFor(() => {
      expect(screen.queryByRole("note")).toBeNull();
    });
  });

  it("a save already under way when the text went in does not let the copy go", async () => {
    const server = serve([entry()]);
    const onReplace = vi.fn(() => true);
    // A save started before the replace and has not landed yet.
    const inFlight = startSave();
    const { rerender } = mount({ onReplace });
    await userEvent.click(
      await screen.findByRole("button", { name: "Replace with it" }),
    );
    await within(note()).findByText("Waiting for the save.");
    saves.landed = inFlight;
    rerender({ onReplace });
    await act(async () => {
      await Promise.resolve();
    });
    expect(server.dropped).toEqual([]);
    expect(within(note()).getByText("Waiting for the save.")).toBeVisible();
    // The next save began after it, and does.
    saves.landed = startSave();
    rerender({ onReplace });
    await waitFor(() => {
      expect(server.dropped).toEqual(["k1"]);
    });
  });

  it("in a room, says the text is back and never drops the kept copy on its own", async () => {
    const server = serve([entry()]);
    const onReplace = vi.fn(() => true);
    const { rerender } = mount({ onReplace, soloSaves: undefined });
    await userEvent.click(
      await screen.findByRole("button", { name: "Replace with it" }),
    );
    expect(onReplace).toHaveBeenCalledWith(KEPT);
    expect(await within(note()).findByRole("status")).toHaveTextContent(
      "Your text is back in the editor. Drop the kept copy once it is saved.",
    );
    expect(within(note()).queryByText("Waiting for the save.")).toBeNull();
    expect(
      within(note()).queryByRole("button", { name: "Replace with it" }),
    ).toBeNull();
    // Whatever the room does next, the note never claims a save.
    rerender({ onReplace, soloSaves: undefined });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(server.dropped).toEqual([]);
  });

  it("in a room, Drop after Replace with it drops the kept copy", async () => {
    const server = serve([entry()]);
    const onDropped = vi.fn();
    mount({ soloSaves: undefined, onDropped });
    await userEvent.click(
      await screen.findByRole("button", { name: "Replace with it" }),
    );
    await within(note()).findByText(
      "Your text is back in the editor. Drop the kept copy once it is saved.",
    );
    await userEvent.click(
      within(note()).getByRole("button", { name: "Drop the kept text" }),
    );
    await userEvent.click(
      within(note()).getByRole("button", { name: "Drop it" }),
    );
    await waitFor(() => {
      expect(server.dropped).toEqual(["k1"]);
    });
    expect(onDropped).toHaveBeenCalled();
    await waitFor(() => {
      expect(screen.queryByRole("note")).toBeNull();
    });
  });

  it("drops nothing and says so when the text does not reach the editor", async () => {
    const server = serve([entry()]);
    mount({ onReplace: () => false });
    await userEvent.click(
      await screen.findByRole("button", { name: "Replace with it" }),
    );
    expect(await within(note()).findByRole("alert")).toHaveTextContent(
      "The text did not reach the editor. Nothing changed.",
    );
    expect(server.dropped).toEqual([]);
    expect(
      within(note()).getByRole("button", { name: "Replace with it" }),
    ).toBeVisible();
  });

  it("changes nothing when the room ends while the text is on its way", async () => {
    const server = serve([entry()]);
    let answer: (value: unknown) => void = () => undefined;
    server.read = () =>
      new Promise((resolve) => {
        answer = resolve;
      });
    const onReplace = vi.fn(() => true);
    const { rerender } = mount({ onReplace });
    await userEvent.click(
      await screen.findByRole("button", { name: "Replace with it" }),
    );
    // The room goes away before the read answers.
    rerender({ onReplace, ready: false });
    await act(async () => {
      answer({ domain: "eng", ...entry(), text: KEPT });
      await Promise.resolve();
    });
    expect(await within(note()).findByRole("alert")).toHaveTextContent(
      "The editor cannot take the text now. Nothing changed.",
    );
    expect(onReplace).not.toHaveBeenCalled();
    expect(server.dropped).toEqual([]);
    expect(note()).toBeVisible();
  });

  it("offers no replace while the editor cannot take a text", async () => {
    serve([entry()]);
    mount({ ready: false });
    await screen.findByRole("note", { name: "Kept text" });
    expect(
      within(note()).queryByRole("button", { name: "Replace with it" }),
    ).toBeNull();
    expect(
      within(note()).getByRole("button", { name: "Drop the kept text" }),
    ).toBeVisible();
  });

  it("says why a read failed and changes nothing", async () => {
    const server = serve([entry()]);
    server.read = () =>
      Promise.reject(new ApiProblem(500, "internal", "the disk is gone"));
    const onReplace = vi.fn(() => true);
    mount({ onReplace });
    await userEvent.click(
      await screen.findByRole("button", { name: "Replace with it" }),
    );
    expect(await within(note()).findByRole("alert")).toHaveTextContent(
      "the disk is gone",
    );
    expect(onReplace).not.toHaveBeenCalled();
    expect(server.dropped).toEqual([]);
  });

  it("goes quietly when the text is already gone", async () => {
    const server = serve([entry()]);
    let listed = [entry()];
    server.read = () => {
      listed = [];
      return Promise.reject(new ApiProblem(404, "not found", "no kept text"));
    };
    const base = apiMock.getMockImplementation();
    apiMock.mockImplementation((path, init) =>
      path === "/domains/eng/kept"
        ? Promise.resolve({ domain: "eng", kept: listed } as never)
        : (base as NonNullable<typeof base>)(path, init),
    );
    mount();
    await userEvent.click(
      await screen.findByRole("button", { name: "Replace with it" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("note")).toBeNull();
    });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("keeps the text in the editor and offers no second replace when the drop after the save fails", async () => {
    const server = serve([entry()]);
    server.remove = () =>
      Promise.reject(new ApiProblem(500, "internal", "the disk is gone"));
    const onReplace = vi.fn(() => true);
    const { rerender } = mount({ onReplace });
    await userEvent.click(
      await screen.findByRole("button", { name: "Replace with it" }),
    );
    await within(note()).findByText("Waiting for the save.");
    saves.landed = startSave();
    rerender({ onReplace });
    expect(await within(note()).findByRole("alert")).toHaveTextContent(
      "Your text is saved, but the kept copy could not be dropped. the disk is gone",
    );
    expect(within(note()).queryByText("Waiting for the save.")).toBeNull();
    expect(
      within(note()).queryByRole("button", { name: "Replace with it" }),
    ).toBeNull();
    expect(onReplace).toHaveBeenCalledTimes(1);
  });

  it("Drop asks in a group, drops, and hands the focus back to the editor", async () => {
    const server = serve([entry()]);
    const onDropped = vi.fn();
    mount({ onDropped });
    await userEvent.click(
      await screen.findByRole("button", { name: "Drop the kept text" }),
    );
    const group = within(note()).getByRole("group", {
      name: "Drop it for good?",
    });
    expect(
      within(group).getByRole("button", { name: "Drop it" }),
    ).toHaveFocus();
    await userEvent.click(
      within(group).getByRole("button", { name: "Drop it" }),
    );
    await waitFor(() => {
      expect(server.dropped).toEqual(["k1"]);
    });
    expect(onDropped).toHaveBeenCalled();
  });

  it("Keep and Escape take the question back and put the focus on Drop", async () => {
    const server = serve([entry()]);
    mount();
    await userEvent.click(
      await screen.findByRole("button", { name: "Drop the kept text" }),
    );
    await userEvent.click(within(note()).getByRole("button", { name: "Keep" }));
    expect(
      within(note()).getByRole("button", { name: "Drop the kept text" }),
    ).toHaveFocus();
    await userEvent.click(
      within(note()).getByRole("button", { name: "Drop the kept text" }),
    );
    await userEvent.keyboard("{Escape}");
    expect(within(note()).queryByRole("group")).toBeNull();
    expect(
      within(note()).getByRole("button", { name: "Drop the kept text" }),
    ).toHaveFocus();
    expect(server.dropped).toEqual([]);
  });

  it("a failed drop says why and puts the focus back on Drop", async () => {
    const server = serve([entry()]);
    server.remove = () =>
      Promise.reject(new ApiProblem(500, "internal", "the disk is gone"));
    const onDropped = vi.fn();
    mount({ onDropped });
    await userEvent.click(
      await screen.findByRole("button", { name: "Drop the kept text" }),
    );
    await userEvent.click(
      within(note()).getByRole("button", { name: "Drop it" }),
    );
    expect(await within(note()).findByRole("alert")).toHaveTextContent(
      "the disk is gone",
    );
    await waitFor(() => {
      expect(
        within(note()).getByRole("button", { name: "Drop the kept text" }),
      ).toHaveFocus();
    });
    expect(onDropped).not.toHaveBeenCalled();
  });

  it("only shows the text on a read-only instance", async () => {
    serve([entry()]);
    mount({}, true);
    await screen.findByRole("note", { name: "Kept text" });
    expect(within(note()).getByRole("button", { name: "Show" })).toBeVisible();
    expect(
      within(note()).queryByRole("button", { name: "Replace with it" }),
    ).toBeNull();
    expect(
      within(note()).queryByRole("button", { name: "Drop the kept text" }),
    ).toBeNull();
  });

  it("Show opens the text with a Copy, and Close gives the focus back to Show", async () => {
    serve([entry()]);
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    mount();
    const show = await screen.findByRole("button", { name: "Show" });
    await userEvent.click(show);
    const dialog = await screen.findByRole("dialog", { name: "Kept text" });
    const text = await within(dialog).findByLabelText("The kept text");
    expect(text).toHaveAttribute("tabindex", "0");
    await userEvent.click(within(dialog).getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith(KEPT);
    expect(await within(dialog).findByText("Copied")).toBeVisible();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Close" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    await waitFor(() => {
      expect(show).toHaveFocus();
    });
  });
});
