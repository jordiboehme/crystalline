/**
 * Renaming a domain, from its own page.
 *
 * What is pinned here is the request the dialog builds - both `name` and
 * `local_only` always, whichever way the checkbox sits - and that a refusal
 * arrives in the server's own words with the form still standing and the
 * checkbox exactly where the reader left it, the way every dialog on this
 * app answers a 409 or a 422.
 */

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { MemoryRouter } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import App from "../App";
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

/** The domain page's own furniture, stubbed the way `DomainHome`'s is. */
function domainFixtures(name: string): Record<string, Answer> {
  return {
    [`/domains/${name}/manifest`]: () => ({
      domain: name,
      markdown: `# ${name}`,
    }),
    [`/domains/${name}/tree`]: () => ({
      domain: name,
      path: "/",
      folders: [],
      engrams: [],
    }),
    [`/domains/${name}/engrams`]: () => ({
      mode: "text",
      total: 0,
      page: 1,
      limit: 50,
      count: 0,
      hits: [],
    }),
    [`/vocabulary`]: () => ({
      domain: name,
      tags: [],
      categories: [],
      relation_types: [],
    }),
    [`/domains/${name}/members`]: () => ({
      owner: null,
      visibility: "shared",
      members: [],
    }),
  };
}

function serveAs(
  role: "admin" | "editor",
  routes: Record<string, Answer> = {},
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role }) }),
      "/domains": domainsResponse,
      ...domainFixtures("eng"),
      ...routes,
    }),
  );
}

/** The body of the request the app sent to `path` with `method`, parsed. */
function sentBody(path: string, method: string): unknown {
  const call = apiMock.mock.calls.find(
    ([sent, init]) => sent === path && init?.method === method,
  );
  if (!call) {
    throw new Error(`no ${method} to ${path}`);
  }
  const body = call[1]?.body;
  if (typeof body !== "string") {
    throw new Error(`the ${method} to ${path} carried no JSON body`);
  }
  return JSON.parse(body) as unknown;
}

async function openDialog(): Promise<HTMLElement> {
  await userEvent.click(
    await screen.findByRole("button", { name: "Rename domain" }),
  );
  return screen.findByRole("dialog", { name: "Rename domain" });
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the rename dialog", () => {
  it("submits the new name with local_only false when the checkbox is left alone", async () => {
    const renamed = vi.fn(() => ({
      domain: "engineering",
      previous: "eng",
      local_only: false,
      manifest_written: true,
      manifest_draft: false,
      rewritten: [],
      left_behind: [],
      aliases: ["eng"],
      shadows: [],
    }));
    serveAs("admin", {
      "/domains/eng/rename": (_path, init) => {
        if (init?.method !== "POST") {
          throw new ApiProblem(404, "not found", "no stub for GET");
        }
        return renamed();
      },
      ...domainFixtures("engineering"),
    });
    renderApp("/d/eng");

    const dialog = await openDialog();
    await userEvent.type(
      within(dialog).getByLabelText("New name"),
      "engineering",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Rename" }),
    );

    await waitFor(() => {
      expect(renamed).toHaveBeenCalled();
    });
    expect(sentBody("/domains/eng/rename", "POST")).toEqual({
      name: "engineering",
      local_only: false,
    });
    // The flow lands where the domain now is.
    expect(
      await screen.findByRole("heading", { level: 1, name: "engineering" }),
    ).toBeVisible();
  });

  it("toggles local_only on when the checkbox is checked", async () => {
    const renamed = vi.fn(() => ({
      domain: "engineering",
      previous: "eng",
      local_only: true,
      manifest_written: false,
      manifest_draft: false,
      rewritten: [],
      left_behind: [],
      aliases: ["eng"],
      shadows: [],
    }));
    serveAs("admin", {
      "/domains/eng/rename": (_path, init) => {
        if (init?.method !== "POST") {
          throw new ApiProblem(404, "not found", "no stub for GET");
        }
        return renamed();
      },
      ...domainFixtures("engineering"),
    });
    renderApp("/d/eng");

    const dialog = await openDialog();
    await userEvent.type(
      within(dialog).getByLabelText("New name"),
      "engineering",
    );
    await userEvent.click(
      within(dialog).getByRole("checkbox", { name: "This machine only" }),
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Rename" }),
    );

    await waitFor(() => {
      expect(sentBody("/domains/eng/rename", "POST")).toEqual({
        name: "engineering",
        local_only: true,
      });
    });
  });

  it("shows a 409 refusal in the server's own words, with the form untouched", async () => {
    serveAs("admin", {
      "/domains/eng/rename": (_path, init) => {
        if (init?.method !== "POST") {
          throw new ApiProblem(404, "not found", "no stub for GET");
        }
        throw new ApiProblem(
          409,
          "conflict",
          "'engineering' is already a domain here; pick another name",
        );
      },
    });
    renderApp("/d/eng");

    const dialog = await openDialog();
    await userEvent.type(
      within(dialog).getByLabelText("New name"),
      "engineering",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Rename" }),
    );

    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(/already a domain here/);
    // The dialog stays open and nothing was reset: the fix is a different
    // name, not everything over again.
    expect(within(dialog).getByLabelText("New name")).toHaveValue(
      "engineering",
    );
    expect(
      within(dialog).getByRole("checkbox", { name: "This machine only" }),
    ).not.toBeChecked();
    expect(screen.getByRole("dialog", { name: "Rename domain" })).toBeVisible();
  });

  it("shows a 422 naming This machine only without ticking the checkbox itself", async () => {
    serveAs("admin", {
      "/domains/eng/rename": (_path, init) => {
        if (init?.method !== "POST") {
          throw new ApiProblem(404, "not found", "no stub for GET");
        }
        throw new ApiProblem(
          422,
          "invalid",
          "domain 'eng' has no MANIFEST to write the new name into; to rename it on this machine only, run `crystalline domain rename eng engineering --local` or pick This machine only in Rename on the domain page, which leaves the MANIFEST and the links as they are",
        );
      },
    });
    renderApp("/d/eng");

    const dialog = await openDialog();
    await userEvent.type(
      within(dialog).getByLabelText("New name"),
      "engineering",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Rename" }),
    );

    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(/This machine only/);
    expect(
      within(dialog).getByRole("checkbox", { name: "This machine only" }),
    ).not.toBeChecked();
  });

  it("carries the rewritten counts and left-behind list to the renamed page, and survives the listing's own later read turning over", async () => {
    // A domain whose MANIFEST already declares the name this rename lines
    // it up with - the "shadowed" case a rename most often fixes - so the
    // listing the reader started on already names both spellings, the way a
    // real one would before the POST lands. It only turns over to the
    // renamed shape once the POST itself has answered, matching what a real
    // server would say from that point on: this is what the dialog's own
    // background `invalidateQueries` reads when it lands, after the cache
    // patch and the navigation have already run inline.
    let done = false;
    let domainsReads = 0;
    const listing = () => {
      domainsReads += 1;
      return done
        ? {
            behavior: [],
            domains: [
              {
                name: "engineering",
                kind: "file",
                engrams: 4,
                when_to_use: [],
                canonical_name: "engineering",
                aliases: ["eng"],
                name_origin: "explicit",
                shadowed: false,
                renaming: false,
              },
            ],
          }
        : {
            behavior: [],
            domains: [
              {
                name: "eng",
                kind: "file",
                engrams: 4,
                when_to_use: [],
                canonical_name: "engineering",
                aliases: [],
                name_origin: "explicit",
                shadowed: false,
                renaming: false,
              },
            ],
          };
    };

    serveAs("admin", {
      "/domains": listing,
      "/domains/eng/rename": (_path, init) => {
        if (init?.method !== "POST") {
          throw new ApiProblem(404, "not found", "no stub for GET");
        }
        done = true;
        return {
          domain: "engineering",
          previous: "eng",
          local_only: false,
          manifest_written: true,
          manifest_draft: true,
          rewritten: [{ domain: "ops", engrams: 2, references: 3 }],
          left_behind: [{ domain: "archive", path: "old.md", references: 1 }],
          aliases: ["eng"],
          shadows: [],
        };
      },
      ...domainFixtures("engineering"),
    });
    renderApp("/d/eng");
    await screen.findByRole("heading", { level: 1, name: "eng" });

    const dialog = await openDialog();
    const before = domainsReads;
    await userEvent.type(
      within(dialog).getByLabelText("New name"),
      "engineering",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Rename" }),
    );

    expect(
      await screen.findByRole("heading", { level: 1, name: "engineering" }),
    ).toBeVisible();
    expect(screen.getByText("Renamed from eng.")).toBeVisible();
    expect(
      screen.getByText("The MANIFEST change waits for review."),
    ).toBeVisible();
    expect(
      screen.getByText("ops: 2 engrams rewritten, 3 references."),
    ).toBeVisible();
    expect(screen.getByText("Links left as they were:")).toBeVisible();
    expect(screen.getByText("archive/old.md (1 reference)")).toBeVisible();
    // Not a trivial re-check of a heading `findByRole` already found: this
    // waits for an ACTUAL later read of "/domains" to land - the dialog's
    // own background invalidate, reading the listing the POST above turned
    // over - and only then re-asserts. A page that quietly bounced back to
    // "eng" once that real refetch landed would fail here; one that merely
    // held onto its first render would not have been caught by this at all.
    await waitFor(() => {
      expect(domainsReads).toBeGreaterThan(before);
    });
    expect(
      screen.getByRole("heading", { level: 1, name: "engineering" }),
    ).toBeVisible();
    expect(screen.getByText("Renamed from eng.")).toBeVisible();
  });

  it("keeps the rename summary through StrictMode's double-invoked effects", async () => {
    // The same shape as the test above, wrapped in <StrictMode> - what
    // `main.tsx` actually renders the whole app inside. StrictMode's
    // dev-only double-invoke (setup, cleanup, setup, right after the very
    // first mount) is exactly what broke an earlier version of the summary:
    // its cleanup cleared the slot unconditionally, before the domain page's
    // OTHER queries (manifest, members, tags, sync status) had even settled,
    // so the very next of their re-renders read an already-empty slot.
    let done = false;
    const listing = () =>
      done
        ? {
            behavior: [],
            domains: [
              {
                name: "engineering",
                kind: "file",
                engrams: 4,
                when_to_use: [],
                canonical_name: "engineering",
                aliases: ["eng"],
                name_origin: "explicit",
                shadowed: false,
                renaming: false,
              },
            ],
          }
        : {
            behavior: [],
            domains: [
              {
                name: "eng",
                kind: "file",
                engrams: 4,
                when_to_use: [],
                canonical_name: "engineering",
                aliases: [],
                name_origin: "explicit",
                shadowed: false,
                renaming: false,
              },
            ],
          };

    serveAs("admin", {
      "/domains": listing,
      "/domains/eng/rename": (_path, init) => {
        if (init?.method !== "POST") {
          throw new ApiProblem(404, "not found", "no stub for GET");
        }
        done = true;
        return {
          domain: "engineering",
          previous: "eng",
          local_only: false,
          manifest_written: true,
          manifest_draft: true,
          rewritten: [{ domain: "ops", engrams: 2, references: 3 }],
          left_behind: [],
          aliases: ["eng"],
          shadows: [],
        };
      },
      ...domainFixtures("engineering"),
    });

    render(
      <StrictMode>
        <MemoryRouter initialEntries={["/d/eng"]}>
          <App />
        </MemoryRouter>
      </StrictMode>,
    );
    await screen.findByRole("heading", { level: 1, name: "eng" });

    const dialog = await openDialog();
    await userEvent.type(
      within(dialog).getByLabelText("New name"),
      "engineering",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Rename" }),
    );

    expect(
      await screen.findByRole("heading", { level: 1, name: "engineering" }),
    ).toBeVisible();
    expect(screen.getByText("Renamed from eng.")).toBeVisible();

    // Let this page's OTHER queries settle and force further re-renders -
    // exactly what exposed the StrictMode bug, since none of them touch
    // the rename summary directly. The summary must still be there once
    // they have.
    await screen.findByRole("heading", { name: "Members" });
    expect(screen.getByText("Renamed from eng.")).toBeVisible();
  });

  it("does not replay the summary after a detour through a folder and back", async () => {
    // A domain with one folder, so there is somewhere ordinary to browse
    // into: `DomainHome` swaps `DomainPage` for `FolderPage` on the way in,
    // an outright unmount of the component that holds the summary's state.
    serveAs("admin", {
      "/domains/eng/rename": (_path, init) => {
        if (init?.method !== "POST") {
          throw new ApiProblem(404, "not found", "no stub for GET");
        }
        return {
          domain: "engineering",
          previous: "eng",
          local_only: false,
          manifest_written: true,
          manifest_draft: false,
          rewritten: [],
          left_behind: [],
          aliases: ["eng"],
          shadows: [],
        };
      },
      "/domains/engineering/manifest": () => ({
        domain: "engineering",
        markdown: "# engineering",
      }),
      "/domains/engineering/tree": (path) =>
        path.includes("path=notes")
          ? { domain: "engineering", path: "notes", folders: [], engrams: [] }
          : {
              domain: "engineering",
              path: "/",
              folders: ["notes"],
              engrams: [],
            },
      "/domains/engineering/engrams": () => ({
        mode: "text",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
      "/vocabulary": () => ({
        domain: "engineering",
        tags: [],
        categories: [],
        relation_types: [],
      }),
      "/domains/engineering/members": () => ({
        owner: null,
        visibility: "shared",
        members: [],
      }),
    });
    renderApp("/d/eng");
    await screen.findByRole("heading", { level: 1, name: "eng" });

    const dialog = await openDialog();
    await userEvent.type(
      within(dialog).getByLabelText("New name"),
      "engineering",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Rename" }),
    );

    expect(
      await screen.findByRole("heading", { level: 1, name: "engineering" }),
    ).toBeVisible();
    expect(screen.getByText("Renamed from eng.")).toBeVisible();

    // Browse into the folder: an ordinary detour, and a real unmount of the
    // component whose state the summary now lives in.
    await userEvent.click(await screen.findByRole("button", { name: "notes" }));
    await screen.findByRole("heading", { level: 1, name: /notes/ });
    expect(screen.queryByText("Renamed from eng.")).toBeNull();

    // Back to the domain's own home - a FRESH mount of that component,
    // with no memory of the earlier one - by the breadcrumb the folder
    // page's own heading carries.
    const folderHeading = screen.getByRole("heading", { level: 1 });
    await userEvent.click(
      within(folderHeading).getByRole("link", { name: "engineering" }),
    );

    expect(
      await screen.findByRole("heading", { level: 1, name: "engineering" }),
    ).toBeVisible();
    // The cache slot was already emptied the first time this domain's page
    // showed the summary, so this later, unrelated mount finds nothing left
    // to replay.
    expect(screen.queryByText("Renamed from eng.")).toBeNull();
  });

  it("shows only the latest summary when a domain is renamed twice in a row", async () => {
    serveAs("admin", {
      "/domains/eng/rename": (_path, init) => {
        if (init?.method !== "POST") {
          throw new ApiProblem(404, "not found", "no stub for GET");
        }
        return {
          domain: "engineering",
          previous: "eng",
          local_only: false,
          manifest_written: false,
          manifest_draft: false,
          rewritten: [{ domain: "ops", engrams: 1, references: 1 }],
          left_behind: [],
          aliases: ["eng"],
          shadows: [],
        };
      },
      "/domains/engineering/rename": (_path, init) => {
        if (init?.method !== "POST") {
          throw new ApiProblem(404, "not found", "no stub for GET");
        }
        return {
          domain: "engineering-team",
          previous: "engineering",
          local_only: false,
          manifest_written: false,
          manifest_draft: false,
          rewritten: [{ domain: "docs", engrams: 9, references: 9 }],
          left_behind: [],
          aliases: ["engineering", "eng"],
          shadows: [],
        };
      },
      ...domainFixtures("engineering"),
      ...domainFixtures("engineering-team"),
    });
    renderApp("/d/eng");
    await screen.findByRole("heading", { level: 1, name: "eng" });

    const first = await openDialog();
    await userEvent.type(
      within(first).getByLabelText("New name"),
      "engineering",
    );
    await userEvent.click(
      within(first).getByRole("button", { name: "Rename" }),
    );
    expect(
      await screen.findByRole("heading", { level: 1, name: "engineering" }),
    ).toBeVisible();
    expect(screen.getByText("Renamed from eng.")).toBeVisible();
    expect(
      screen.getByText("ops: 1 engram rewritten, 1 reference."),
    ).toBeVisible();

    // Renamed again immediately, on the very page the first rename just
    // landed on - the same continuously-mounted instance, not a detour.
    const second = await openDialog();
    await userEvent.type(
      within(second).getByLabelText("New name"),
      "engineering-team",
    );
    await userEvent.click(
      within(second).getByRole("button", { name: "Rename" }),
    );

    expect(
      await screen.findByRole("heading", {
        level: 1,
        name: "engineering-team",
      }),
    ).toBeVisible();
    // Only the second summary: its own report, not the first one's summary
    // sitting alongside it or having leaked through.
    expect(screen.getByText("Renamed from engineering.")).toBeVisible();
    expect(
      screen.getByText("docs: 9 engrams rewritten, 9 references."),
    ).toBeVisible();
    expect(screen.queryByText("Renamed from eng.")).toBeNull();
    expect(
      screen.queryByText("ops: 1 engram rewritten, 1 reference."),
    ).toBeNull();
  });
});
