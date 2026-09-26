/**
 * Renaming a domain, from its own page.
 *
 * What is pinned here is the request the dialog builds - both `name` and
 * `local_only` always, whichever way the checkbox sits - and that a refusal
 * arrives in the server's own words with the form still standing and the
 * checkbox exactly where the reader left it, the way every dialog on this
 * app answers a 409 or a 422.
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

  it("carries the rewritten counts and left-behind list to the renamed page, without bouncing back once the listing catches up", async () => {
    // A domain whose MANIFEST already declares the name this rename lines
    // it up with - the "shadowed" case a rename most often fixes - so the
    // listing the reader started on already names both spellings, the way a
    // real one would before the POST lands.
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
        // The listing only turns over once the rename itself has answered,
        // the way the real invalidate-then-refetch does: the dialog awaits
        // it before it navigates, so the redirect above every screen never
        // reads the stale row.
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
    expect(
      screen.getByText("The MANIFEST change waits for review."),
    ).toBeVisible();
    expect(
      screen.getByText("ops: 2 engrams rewritten, 3 references."),
    ).toBeVisible();
    expect(screen.getByText("Links left as they were:")).toBeVisible();
    expect(screen.getByText("archive/old.md (1 reference)")).toBeVisible();
    // Nothing sends the reader back to the old address: by the time the
    // redirect above every screen next looks at the listing, "eng" already
    // reads as an alias of "engineering" rather than its own local name.
    await waitFor(() => {
      expect(
        screen.getByRole("heading", { level: 1, name: "engineering" }),
      ).toBeVisible();
    });
  });
});
