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

function tree(path: string) {
  return path.includes("path=notes")
    ? { domain: "eng", folders: [], engrams: [] }
    : { domain: "eng", folders: ["notes"], engrams: [] };
}

beforeEach(() => {
  apiMock.mockReset();
  localStorage.clear();
});

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
});
