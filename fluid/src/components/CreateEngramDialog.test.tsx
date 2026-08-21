import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import { useCollabSession } from "../collab/useCollabSession";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  soloCollabSession,
  userFixture,
} from "../test/harness";

// Above roughly load average 33 this file's slower tests exceed the 5000 ms
// default (a threshold effect measured 2026-08-14, plans history); the raise
// keeps a loaded machine from reading as a failure. Never raise the global
// default to hide this.
vi.setConfig({ testTimeout: 15000 });

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

// Creating lands on the editor route; what happens there is the editor's own
// test. This flow opens it on the solo surface, with no session to wait for.
vi.mock("../collab/useCollabSession", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("../collab/useCollabSession")>();
  return { ...actual, useCollabSession: vi.fn() };
});

const apiMock = vi.mocked(api);
const collabMock = vi.mocked(useCollabSession);

function tree(path: string) {
  return path.includes("path=notes")
    ? { domain: "eng", folders: [], engrams: [] }
    : { domain: "eng", folders: ["notes"], engrams: [] };
}

beforeEach(() => {
  apiMock.mockReset();
  collabMock.mockReset();
  collabMock.mockReturnValue(soloCollabSession());
  localStorage.clear();
});

/** Every read of this domain's tree, in order. */
function trees(): string[] {
  return apiMock.mock.calls
    .map(([path]) => path)
    .filter((path) => path.startsWith("/domains/eng/tree"));
}

/** Every read of the vocabulary route, in order. */
function vocabularyReads(): string[] {
  return apiMock.mock.calls
    .map(([path]) => path)
    .filter((path) => path.startsWith("/vocabulary"));
}

/** The tag names the dialog offers, in the order it offers them. */
function suggestedTags(): string[] {
  return [...(document.getElementById("create-tags")?.children ?? [])].map(
    (option) => option.getAttribute("value") ?? "",
  );
}

describe("the create flow", () => {
  it("creates in the picked folder and lands in the editor", async () => {
    const created = vi.fn(() => ({
      domain: "eng",
      permalink: "notes/fresh-thought",
      title: "Fresh Thought",
      content: "---\ntitle: Fresh Thought\n---\n\n",
      checksum: "new111",
      frontmatter: {},
      observations: [],
      relations: [],
      links: [],
    }));
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture() }),
        "/domains": domainsResponse,
        "/domains/eng/manifest": () => ({
          domain: "eng",
          markdown: "# eng",
          checksum: "m1",
        }),
        "/domains/eng/tree": (path) => tree(path),
        "/domains/eng/engrams": (_path, init) =>
          init?.method === "POST"
            ? created()
            : { total: 0, page: 1, limit: 50, count: 0, hits: [] },
        "/domains/eng/engrams/notes/fresh-thought": () => created(),
        "/validate": () => ({ findings: [], errors: 0 }),
        "/vocabulary": () => ({ tags: [], categories: [], relation_types: [] }),
        "/graph": () => ({ nodes: [], edges: [], truncated: false, hidden: 0 }),
      }),
    );
    renderApp("/d/eng");
    await userEvent.click(
      await screen.findByRole("button", { name: "New engram" }),
    );
    const dialog = await screen.findByRole("dialog", { name: /new engram/i });
    expect(dialog).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("Title"), "Fresh Thought");
    // The folder picker walks the same tree the sidebar reads.
    await userEvent.click(screen.getByRole("radio", { name: "notes" }));
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => {
      expect(created).toHaveBeenCalled();
    });
    const call = apiMock.mock.calls.find(([, init]) => init?.method === "POST");
    const body = call?.[1]?.body;
    if (typeof body !== "string") {
      throw new Error("no POST body");
    }
    expect(JSON.parse(body) as unknown).toMatchObject({
      title: "Fresh Thought",
      folder: "notes",
    });
    // Straight into the editor on what landed.
    expect(await screen.findByLabelText("Engram source")).toBeInTheDocument();
    // The create response seeded the editor's own cache under the same key
    // it reads, so landing on it is never a second round trip for what the
    // POST already answered - `created` backs both the POST and the detail
    // GET, so a second call here would mean a refetch slipped through.
    await new Promise((resolve) => {
      setTimeout(resolve, 50);
    });
    expect(created).toHaveBeenCalledTimes(1);
  });

  it("surfaces a permalink collision in the server's words", async () => {
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture() }),
        "/domains": domainsResponse,
        "/domains/eng/manifest": () => ({
          domain: "eng",
          markdown: "",
          checksum: "m1",
        }),
        "/domains/eng/tree": (path) => tree(path),
        "/domains/eng/engrams": (_path, init) => {
          if (init?.method === "POST") {
            throw new ApiProblem(
              409,
              "conflict",
              "an engram 'fresh-thought' already exists in 'eng'",
            );
          }
          return { total: 0, page: 1, limit: 50, count: 0, hits: [] };
        },
        "/vocabulary": () => ({ tags: [], categories: [], relation_types: [] }),
      }),
    );
    renderApp("/d/eng");
    await userEvent.click(
      await screen.findByRole("button", { name: "New engram" }),
    );
    await userEvent.type(screen.getByLabelText("Title"), "Fresh Thought");
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(
      await screen.findByText(/already exists in 'eng'/),
    ).toBeInTheDocument();
  });

  it("sends trimmed comma-separated tags with the create", async () => {
    const created = vi.fn(() => ({
      domain: "eng",
      permalink: "notes/gamma",
      title: "Gamma",
      content: "---\ntitle: Gamma\n---\n\n",
      checksum: "new222",
      frontmatter: {},
      observations: [],
      relations: [],
      links: [],
    }));
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture() }),
        "/domains": domainsResponse,
        "/domains/eng/manifest": () => ({
          domain: "eng",
          markdown: "# eng",
          checksum: "m1",
        }),
        "/domains/eng/tree": (path) => tree(path),
        "/domains/eng/engrams": (_path, init) =>
          init?.method === "POST"
            ? created()
            : { total: 0, page: 1, limit: 50, count: 0, hits: [] },
        "/domains/eng/engrams/notes/gamma": () => created(),
        "/validate": () => ({ findings: [], errors: 0 }),
        "/vocabulary": () => ({ tags: [], categories: [], relation_types: [] }),
        "/graph": () => ({ nodes: [], edges: [], truncated: false, hidden: 0 }),
      }),
    );
    renderApp("/d/eng");
    await userEvent.click(
      await screen.findByRole("button", { name: "New engram" }),
    );
    await userEvent.type(screen.getByLabelText(/Title/), "Gamma");
    await userEvent.type(
      screen.getByLabelText(/Tags/),
      " rust, collab,,editing ",
    );
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => {
      expect(created).toHaveBeenCalled();
    });
    const call = apiMock.mock.calls.find(([, init]) => init?.method === "POST");
    const body = call?.[1]?.body;
    if (typeof body !== "string") {
      throw new Error("no POST body");
    }
    expect(JSON.parse(body) as unknown).toMatchObject({
      tags: ["rust", "collab", "editing"],
    });
  });

  it("sends the type picked out of the suggestions, glosses and all", async () => {
    const created = vi.fn(() => ({
      domain: "eng",
      permalink: "notes/gamma",
      title: "Gamma",
      content: "---\ntitle: Gamma\n---\n\n",
      checksum: "new222",
      frontmatter: {},
      observations: [],
      relations: [],
      links: [],
    }));
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture() }),
        "/domains": domainsResponse,
        "/domains/eng/manifest": () => ({
          domain: "eng",
          markdown: "# eng",
          checksum: "m1",
        }),
        "/domains/eng/tree": (path) => tree(path),
        "/domains/eng/engrams": (_path, init) =>
          init?.method === "POST"
            ? created()
            : { total: 0, page: 1, limit: 50, count: 0, hits: [] },
        "/domains/eng/engrams/notes/gamma": () => created(),
        "/validate": () => ({ findings: [], errors: 0 }),
        "/vocabulary": () => ({ tags: [], categories: [], relation_types: [] }),
        "/graph": () => ({ nodes: [], edges: [], truncated: false, hidden: 0 }),
      }),
    );
    renderApp("/d/eng");
    await userEvent.click(
      await screen.findByRole("button", { name: "New engram" }),
    );
    await userEvent.type(screen.getByLabelText(/Title/), "Gamma");

    // The words are on the screen with what they mean beside them; nobody has
    // to have memorized the set to write one down. Scoped to the dialog: the
    // domain screen behind it has a Type field of its own, in the filter bar.
    const form = within(screen.getByRole("dialog"));
    await userEvent.click(form.getByLabelText("Type"));
    expect(form.getByRole("option", { name: /^decision/ })).toHaveTextContent(
      "a choice that was made, and why",
    );
    // Escape puts the list away and leaves the form standing: the dialog
    // closes on Escape too, and dismissing a list must not throw away what
    // somebody has been typing into it.
    await userEvent.keyboard("{Escape}");
    expect(form.queryByRole("listbox")).not.toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    await userEvent.click(form.getByLabelText("Type"));
    await userEvent.click(form.getByRole("option", { name: /^decision/ }));
    expect(form.getByLabelText("Type")).toHaveValue("decision");

    // And a word this app has never heard of goes on the wire exactly as
    // typed: the suggestions are the recommended set, not the allowed one.
    await userEvent.type(form.getByLabelText("Status"), "brewing");

    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => {
      expect(created).toHaveBeenCalled();
    });
    const call = apiMock.mock.calls.find(([, init]) => init?.method === "POST");
    const body = call?.[1]?.body;
    if (typeof body !== "string") {
      throw new Error("no POST body");
    }
    expect(JSON.parse(body) as unknown).toMatchObject({
      type: "decision",
      status: "brewing",
    });
  });

  it("offers the words the domain itself writes, with how used they are", async () => {
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture() }),
        "/domains": domainsResponse,
        "/domains/eng/manifest": () => ({
          domain: "eng",
          markdown: "# eng",
          checksum: "m1",
        }),
        "/domains/eng/tree": (path) => tree(path),
        "/domains/eng/engrams": () => ({
          total: 0,
          page: 1,
          limit: 50,
          count: 0,
          hits: [],
        }),
        "/vocabulary": () => ({
          tags: [],
          categories: [],
          relation_types: [],
          types: [
            { name: "playbook", count: 7 },
            { name: "guide", count: 2 },
          ],
          statuses: [{ name: "brewing", count: 5 }],
        }),
      }),
    );
    renderApp("/d/eng");
    await userEvent.click(
      await screen.findByRole("button", { name: "New engram" }),
    );
    const form = within(await screen.findByRole("dialog"));

    await userEvent.click(form.getByLabelText("Type"));
    // A word this app has never heard of, because the domain writes it.
    expect(
      await form.findByRole("option", { name: /^playbook/ }),
    ).toHaveTextContent("7");
    // A recommended word keeps its line and gains the live count.
    const known = form.getByRole("option", { name: /^guide/ });
    expect(known).toHaveTextContent("how to do something, start to finish");
    expect(known).toHaveTextContent("2");

    await userEvent.click(form.getByLabelText("Status"));
    expect(form.getByRole("option", { name: /^brewing/ })).toHaveTextContent(
      "5",
    );
  });

  it("reads the vocabulary once for the whole dialog", async () => {
    // Opened from the engram screen, which reads no vocabulary of its own, so
    // every call counted here is one this dialog made. Tags and the house
    // type/status words are one payload on the wire, not two reads of the
    // same route parsed into different shapes.
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture() }),
        "/domains": domainsResponse,
        "/domains/eng/tree": (path) => tree(path),
        "/domains/eng/engrams/notes/beta": () => ({
          domain: "eng",
          permalink: "notes/beta",
          title: "Beta",
          content: "---\ntitle: Beta\n---\n\n",
          checksum: "b1",
          frontmatter: {},
          observations: [],
          relations: [],
          links: [],
        }),
        "/graph": () => ({ nodes: [], edges: [], truncated: false, hidden: 0 }),
        "/validate": () => ({ findings: [], errors: 0 }),
        "/vocabulary": () => ({
          tags: [
            { name: "rust", engrams: 3 },
            { name: "editing", engrams: 9 },
          ],
          categories: [],
          relation_types: [],
          types: [{ name: "playbook", count: 7 }],
          statuses: [],
        }),
      }),
    );
    renderApp("/d/eng/e/notes/beta");
    await userEvent.click(
      await screen.findByRole("button", { name: "New engram" }),
    );
    await screen.findByRole("dialog", { name: /new engram/i });

    // Commonest first, exactly as the tag input has always ordered them.
    await waitFor(() => {
      expect(suggestedTags()).toEqual(["editing", "rust"]);
    });
    // The same payload backs the house words beside the recommended ones.
    const form = within(screen.getByRole("dialog"));
    await userEvent.click(form.getByLabelText("Type"));
    expect(
      await form.findByRole("option", { name: /^playbook/ }),
    ).toHaveTextContent("7");

    expect(vocabularyReads()).toHaveLength(1);
  });

  it("omits the tags key entirely when the field is left empty", async () => {
    const created = vi.fn(() => ({
      domain: "eng",
      permalink: "notes/gamma",
      title: "Gamma",
      content: "---\ntitle: Gamma\n---\n\n",
      checksum: "new222",
      frontmatter: {},
      observations: [],
      relations: [],
      links: [],
    }));
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture() }),
        "/domains": domainsResponse,
        "/domains/eng/manifest": () => ({
          domain: "eng",
          markdown: "# eng",
          checksum: "m1",
        }),
        "/domains/eng/tree": (path) => tree(path),
        "/domains/eng/engrams": (_path, init) =>
          init?.method === "POST"
            ? created()
            : { total: 0, page: 1, limit: 50, count: 0, hits: [] },
        "/domains/eng/engrams/notes/gamma": () => created(),
        "/validate": () => ({ findings: [], errors: 0 }),
        "/vocabulary": () => ({ tags: [], categories: [], relation_types: [] }),
        "/graph": () => ({ nodes: [], edges: [], truncated: false, hidden: 0 }),
      }),
    );
    renderApp("/d/eng");
    await userEvent.click(
      await screen.findByRole("button", { name: "New engram" }),
    );
    await userEvent.type(screen.getByLabelText(/Title/), "Gamma");
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => {
      expect(created).toHaveBeenCalled();
    });
    // Same lookup the successful-create test above uses: the POST is not
    // necessarily the first call apiMock ever saw (auth, tree and vocabulary
    // reads land first), so find it by method rather than assume an index.
    const call = apiMock.mock.calls.find(([, init]) => init?.method === "POST");
    const body = call?.[1]?.body;
    if (typeof body !== "string") {
      throw new Error("no POST body");
    }
    expect(body).not.toContain('"tags"');
  });

  it("offers the sidebar launcher on the editor route, where the page has none of its own", async () => {
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture() }),
        "/domains": domainsResponse,
        "/domains/eng/tree": (path) => tree(path),
        "/domains/eng/engrams/notes/beta": () => ({
          domain: "eng",
          permalink: "notes/beta",
          title: "Beta",
          content: "---\ntitle: Beta\n---\n\n",
          checksum: "b1",
          frontmatter: {},
          observations: [],
          relations: [],
          links: [],
        }),
        "/graph": () => ({ nodes: [], edges: [], truncated: false, hidden: 0 }),
        "/validate": () => ({ findings: [], errors: 0 }),
      }),
    );
    renderApp("/d/eng/edit/notes/beta");
    await screen.findByLabelText("Engram source");
    // Exactly one launcher: the editor carries no "New engram" of its own,
    // so the only match is the sidebar's, which the permalink derivation for
    // `edit/*` routes (Layout.tsx) is what makes visible here.
    expect(
      await screen.findByRole("button", { name: "New engram" }),
    ).toBeInTheDocument();
  });

  it("moves the tree on, so the sidebar holds what was just created", async () => {
    // Created at the root on purpose: an engram inside a folder opens that
    // folder in the sidebar on the way to the editor, and the level it then
    // fetches would look exactly like the invalidation this is about.
    const created = vi.fn(() => ({
      domain: "eng",
      permalink: "fresh-thought",
      title: "Fresh Thought",
      content: "---\ntitle: Fresh Thought\n---\n\n",
      checksum: "new111",
      frontmatter: {},
      observations: [],
      relations: [],
      links: [],
    }));
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture() }),
        "/domains": domainsResponse,
        "/domains/eng/manifest": () => ({
          domain: "eng",
          markdown: "# eng",
          checksum: "m1",
        }),
        "/domains/eng/tree": (path) => tree(path),
        "/domains/eng/engrams": (_path, init) =>
          init?.method === "POST"
            ? created()
            : { total: 0, page: 1, limit: 50, count: 0, hits: [] },
        "/domains/eng/engrams/fresh-thought": () => created(),
        "/validate": () => ({ findings: [], errors: 0 }),
        "/vocabulary": () => ({ tags: [], categories: [], relation_types: [] }),
        "/graph": () => ({ nodes: [], edges: [], truncated: false, hidden: 0 }),
      }),
    );
    renderApp("/d/eng");
    await userEvent.click(
      await screen.findByRole("button", { name: "New engram" }),
    );
    await userEvent.type(screen.getByLabelText("Title"), "Fresh Thought");
    // The tree is fresh for a minute, so nothing but an invalidation can make
    // it be asked for again - a remount on the way to the editor will not.
    const before = trees().length;
    await userEvent.click(screen.getByRole("button", { name: "Create" }));

    await waitFor(() => {
      expect(created).toHaveBeenCalled();
    });
    // A create is a new row in the tree, so the tree is read again rather
    // than left one engram short until something else happens to move it.
    await waitFor(() => {
      expect(trees().length).toBeGreaterThan(before);
    });
  });
});
