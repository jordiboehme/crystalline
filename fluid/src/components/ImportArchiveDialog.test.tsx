/**
 * Importing an archive into a domain, from the screen it is launched on.
 *
 * The whole point of this dialog is the order of the two calls: an import is
 * the one write in this app that can land hundreds of engrams at once, so what
 * is pinned here is that nothing is written until a dry run has said, entry by
 * entry, what writing would do; that the entry that would not be written says
 * why in the words of whatever refused it; that the collision policy on the
 * wire is the one that was chosen; and that a refusal of the archive itself -
 * a path escaping the domain root - arrives whole, with no table of entries
 * standing beside it as if some of them were fine.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
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

/**
 * The upload, which is two bytes of a zip header and nothing else: the api
 * layer is mocked in this suite, so the archive never has to parse - what the
 * dialog has to get right is that the bytes it read reach the wire.
 */
function archiveFile(): File {
  return new File([new Uint8Array([0x50, 0x4b])], "backup.zip", {
    type: "application/zip",
  });
}

/** The other archive, told apart from the first by its first byte. */
function otherArchiveFile(): File {
  return new File([new Uint8Array([0x42, 0x4b])], "second-thoughts.zip", {
    type: "application/zip",
  });
}

/** A dry run over an archive holding one of each kind of entry. */
function previewReport() {
  return {
    domain: "eng",
    dry_run: true,
    entries: [
      {
        path: "notes/alpha.md",
        status: "new",
        permalink: "notes/alpha",
        findings: [],
      },
      {
        path: "notes/beta.md",
        status: "collides",
        permalink: "notes/beta",
        reason: "an engram already lives at 'notes/beta'",
        findings: [],
      },
      {
        path: "notes/broken.md",
        status: "invalid",
        permalink: null,
        findings: [
          {
            rule: "frontmatter",
            severity: "error",
            message: "missing required field 'type'",
            line: 2,
          },
        ],
      },
      {
        path: "README.txt",
        status: "ignored",
        permalink: null,
        reason: "not markdown",
        findings: [],
      },
    ],
    new: 1,
    collides: 1,
    written: 0,
    skipped: 0,
    invalid: 1,
    ignored: 1,
  };
}

/** What the import that followed it reports back. */
function importReport() {
  return {
    domain: "eng",
    dry_run: false,
    entries: [
      {
        path: "notes/alpha.md",
        status: "created",
        permalink: "notes/alpha",
        findings: [],
      },
      {
        path: "notes/beta.md",
        status: "overwritten",
        permalink: "notes/beta",
        findings: [],
      },
      {
        path: "notes/broken.md",
        status: "invalid",
        permalink: null,
        findings: [
          {
            rule: "frontmatter",
            severity: "error",
            message: "missing required field 'type'",
            line: 2,
          },
        ],
      },
      {
        path: "README.txt",
        status: "ignored",
        permalink: null,
        reason: "not markdown",
        findings: [],
      },
    ],
    new: 0,
    collides: 0,
    written: 2,
    skipped: 0,
    invalid: 1,
    ignored: 1,
  };
}

/** The app as an admin on a domain, with the archive routes a test names. */
function serve(routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "root", role: "admin" }) }),
      "/domains": domainsResponse,
      "/domains/eng/manifest": () => ({ domain: "eng", markdown: "# eng\n" }),
      "/domains/eng/tree": () => ({
        domain: "eng",
        path: "/",
        folders: [],
        engrams: [],
      }),
      "/domains/eng/engrams": () => ({
        mode: "text",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
      "/vocabulary": () => ({
        domain: "eng",
        tags: [],
        categories: [],
        relation_types: [],
      }),
      ...routes,
    }),
  );
}

/** Open the dialog from the domain screen, with the archive already picked. */
async function openWithFile(): Promise<HTMLElement> {
  renderApp("/d/eng");
  const body = await screen.findByRole("main");
  await userEvent.click(
    await within(body).findByRole("button", { name: "Import archive" }),
  );
  const dialog = await screen.findByRole("dialog", { name: /import archive/i });
  await userEvent.upload(
    within(dialog).getByLabelText("Archive file"),
    archiveFile(),
  );
  return dialog;
}

/** Every path the app asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => String(call[0]));
}

/** The request the app sent to `path`, whatever its query string. */
function callTo(path: string): RequestInit {
  const call = apiMock.mock.calls.find(([sent]) =>
    String(sent).startsWith(path),
  );
  if (!call?.[1]) {
    throw new Error(`nothing was sent to ${path}`);
  }
  return call[1];
}

/** The row an entry is drawn in. */
function rowFor(table: HTMLElement, path: string): HTMLElement {
  const row = within(table).getByText(path).closest("tr");
  if (!row) {
    throw new Error(`no row for ${path}`);
  }
  return row;
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("importing an archive", () => {
  it("previews before it commits", async () => {
    serve({ "/domains/eng/archive/preview": () => previewReport() });

    const dialog = await openWithFile();

    // Nothing may be written before something has been dry-run.
    expect(
      within(dialog).getByRole("button", { name: "Import" }),
    ).toBeDisabled();

    await userEvent.click(
      within(dialog).getByRole("button", { name: "Preview" }),
    );

    const table = await within(dialog).findByRole("table", {
      name: /what an import would do/i,
    });
    // One row per entry, each wearing what it would become.
    expect(
      within(rowFor(table, "notes/alpha.md")).getByText("new"),
    ).toBeVisible();
    const collides = rowFor(table, "notes/beta.md");
    expect(within(collides).getByText("collides")).toBeVisible();
    expect(
      within(collides).getByText("an engram already lives at 'notes/beta'"),
    ).toBeVisible();
    // An entry refused by verify says which rule refused it, not just "invalid".
    const invalid = rowFor(table, "notes/broken.md");
    expect(within(invalid).getByText("invalid")).toBeVisible();
    expect(
      within(invalid).getByText("missing required field 'type'"),
    ).toBeVisible();
    expect(
      within(rowFor(table, "README.txt")).getByText("ignored"),
    ).toBeVisible();

    // And what the rows add up to, so the size of the decision is readable
    // without counting rows: this is a dry run, so it counts what would be
    // created and what is already taken.
    expect(within(dialog).getByText("1 new, 1 collide.")).toBeVisible();

    // The dry run went out as the bytes that were picked, announced as a zip.
    const sent = callTo("/domains/eng/archive/preview");
    expect(sent.method).toBe("POST");
    expect(sent.headers).toMatchObject({ "Content-Type": "application/zip" });
    expect(new Uint8Array(sent.body as ArrayBuffer)[0]).toBe(0x50);
    // And it wrote nothing: the second call is the one that does that.
    expect(requested().some((path) => path.includes("/archive/import"))).toBe(
      false,
    );
    expect(
      within(dialog).getByRole("button", { name: "Import" }),
    ).toBeEnabled();
  });

  it("imports with the chosen policy and refreshes what it changed", async () => {
    serve({
      "/domains/eng/archive/preview": () => previewReport(),
      "/domains/eng/archive/import": () => importReport(),
    });

    const dialog = await openWithFile();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Preview" }),
    );
    await within(dialog).findByRole("table", {
      name: /what an import would do/i,
    });
    const before = requested().filter((path) => path === "/domains").length;

    // Skipping what is already there is the default, here as on the server.
    expect(
      within(dialog).getByRole("radio", { name: "Skip existing" }),
    ).toBeChecked();
    await userEvent.click(
      within(dialog).getByRole("radio", { name: "Overwrite existing" }),
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Import" }),
    );

    // What actually happened, in the counters the server tallied.
    expect(
      await within(dialog).findByText(
        /2 written, 0 skipped, 1 invalid, 1 ignored/,
      ),
    ).toBeVisible();
    // The preview's counters go with the preview: what would have happened is
    // not left standing beside what did.
    expect(within(dialog).queryByText(/collide\./)).toBeNull();
    expect(
      requested().includes("/domains/eng/archive/import?policy=overwrite"),
    ).toBe(true);
    // The write announces its bytes for what they are, exactly as the dry run
    // did: the server refuses either route anything else.
    expect(callTo("/domains/eng/archive/import").headers).toMatchObject({
      "Content-Type": "application/zip",
    });
    // An import moves the shape of the domain and its engram count, so both
    // the tree every folder view walks and the listing every sidebar draws are
    // asked again.
    await waitFor(() => {
      expect(
        requested().filter((path) => path.startsWith("/domains/eng/tree"))
          .length,
      ).toBeGreaterThan(1);
      expect(
        requested().filter((path) => path === "/domains").length,
      ).toBeGreaterThan(before);
    });
  });

  it("shows a hygiene refusal whole", async () => {
    serve({
      "/domains/eng/archive/preview": () => {
        throw new ApiProblem(
          422,
          "unprocessable entity",
          "archive entry '../evil.md' escapes the domain root",
        );
      },
    });

    const dialog = await openWithFile();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Preview" }),
    );

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "archive entry '../evil.md' escapes the domain root",
    );
    // The archive was refused as a whole, so no entry of it is reported as
    // fine, and there is still nothing to import.
    expect(within(dialog).queryByRole("table")).toBeNull();
    expect(
      within(dialog).getByRole("button", { name: "Import" }),
    ).toBeDisabled();
  });

  it("drops the report of an archive that is no longer the one picked", async () => {
    // The dry run is held open, because that is the whole window this is
    // about: a real archive is megabytes over the wire, so it is seconds long.
    const gate: { release: () => void } = { release: () => undefined };
    serve({
      "/domains/eng/archive/preview": () =>
        new Promise((resolve) => {
          gate.release = () => {
            resolve(previewReport());
          };
        }),
    });

    const dialog = await openWithFile();
    const preview = within(dialog).getByRole("button", { name: "Preview" });
    await userEvent.click(preview);
    // Second thoughts, mid-flight: another archive, before the first report is
    // back. What comes back is about the file that is no longer on the form.
    await userEvent.upload(
      within(dialog).getByLabelText("Archive file"),
      otherArchiveFile(),
    );
    gate.release();
    // The superseded dry run has landed and settled by the time the button it
    // was disabling comes back.
    await waitFor(() => {
      expect(preview).toBeEnabled();
    });

    // Nothing of the first archive reaches the screen, and nothing arms the
    // write: an Import enabled here would post the SECOND archive's bytes
    // under the first one's report, which is the one thing the two-call design
    // exists to prevent.
    expect(within(dialog).queryByRole("table")).toBeNull();
    expect(
      within(dialog).getByRole("button", { name: "Import" }),
    ).toBeDisabled();
    expect(requested().some((path) => path.includes("/archive/import"))).toBe(
      false,
    );
  });
});
