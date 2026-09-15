/**
 * The review-mode card: which way a write in this domain goes, and the plan
 * that has to be answered before anybody's private drafts end.
 *
 * Mounted through the domain screen the way `MembersCard` is, because what is
 * under test is compositional: the state the card draws comes off the domain
 * listing every screen already reads, not from a route of its own.
 *
 * The property under test throughout: the destructive half is never one press
 * away. Taking review mode off asks the server what it would end, shows every
 * actor and every draft, and sends an answer that names each of them.
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

/** The domain listing, with `eng` reviewing changes or not. */
function listing(reviewing: boolean) {
  const base = domainsResponse();
  return {
    ...base,
    domains: base.domains.map((row) => ({
      ...row,
      review: reviewing ? "overlay" : null,
    })),
  };
}

/** The plan `PUT /domains/eng/review` answers a body with no folds. */
function planResponse(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    mode: "direct",
    review: "overlay",
    applied: false,
    actors: [
      {
        actor: "ada",
        entries: 2,
        drafts: [
          {
            path: "plan.md",
            permalink: "plan",
            tombstone: false,
            conflict: null,
          },
          {
            path: "notes/gone.md",
            permalink: "notes/gone.md",
            tombstone: true,
            conflict: null,
          },
        ],
      },
      {
        actor: "bo",
        entries: 1,
        drafts: [
          {
            path: "rota.md",
            permalink: "rota",
            tombstone: false,
            conflict:
              "the address 'rota' already belongs to other.md in the folder the team shares",
          },
        ],
      },
    ],
    contested_paths: [],
    ...overrides,
  };
}

/** The JSON body a mocked call carried, typed rather than guessed at. */
function sent(init: RequestInit | undefined): {
  mode: string;
  folds?: Record<string, string>;
} {
  const body = init?.body;
  return JSON.parse(typeof body === "string" ? body : "{}") as {
    mode: string;
    folds?: Record<string, string>;
  };
}

function serve(routes: Record<string, Answer> = {}, reviewing = true) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "ada", role: "admin" }) }),
      "/domains": () => listing(reviewing),
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
      "/domains/eng/members": () => ({
        owner: null,
        visibility: "shared",
        members: [],
      }),
      ...routes,
    }),
  );
}

describe("ReviewModeCard", () => {
  beforeEach(() => {
    apiMock.mockReset();
  });

  it("says which way a write goes, and turns the mode on", async () => {
    const calls: unknown[] = [];
    serve(
      {
        "/domains/eng/review": (_path, init) => {
          calls.push(sent(init));
          return {};
        },
      },
      false,
    );
    renderApp("/d/eng");

    const region = await screen.findByRole("region", { name: "Review mode" });
    expect(
      within(region).getByText(/lands in the folder straight away/),
    ).toBeInTheDocument();

    await userEvent.click(
      within(region).getByRole("button", {
        name: "Review changes before they land",
      }),
    );
    await waitFor(() => {
      expect(calls).toEqual([{ mode: "overlay" }]);
    });
  });

  it("asks what leaving would end before it ends anything", async () => {
    const calls: unknown[] = [];
    serve({
      "/domains/eng/review": (_path, init) => {
        const body = sent(init);
        calls.push(body);
        return body.folds === undefined ? planResponse() : {};
      },
    });
    renderApp("/d/eng");

    const region = await screen.findByRole("region", { name: "Review mode" });
    await userEvent.click(
      within(region).getByRole("button", { name: "Take review mode off" }),
    );

    // The plan, not the change: every actor, every draft, and the deletion said
    // to be one.
    await screen.findByText(/ada \(2\)/);
    expect(screen.getByText(/drafted plan.md/)).toBeInTheDocument();
    expect(screen.getByText(/deleted notes\/gone.md/)).toBeInTheDocument();
    expect(screen.getByText(/already belongs to other.md/)).toBeInTheDocument();
    expect(calls).toEqual([{ mode: "direct" }]);

    // One answer per actor, and the one that ends somebody's work is chosen
    // rather than defaulted into.
    await userEvent.click(
      screen.getAllByRole("radio", { name: "End them" })[1],
    );
    await userEvent.click(
      screen.getAllByRole("button", { name: "Take review mode off" })[0],
    );
    await waitFor(() => {
      expect(calls[1]).toEqual({
        mode: "direct",
        folds: { ada: "fold", bo: "discard" },
      });
    });
  });

  it("shows the server's refusal in the server's own words", async () => {
    serve({
      "/domains/eng/review": (_path, init) => {
        const body = sent(init);
        if (body.folds === undefined) {
          return planResponse();
        }
        throw new ApiProblem(
          409,
          "Conflict",
          "'ada' and 'bo' are both drafting plan.md in domain 'eng', and only one of them can be the file",
        );
      },
    });
    renderApp("/d/eng");

    const region = await screen.findByRole("region", { name: "Review mode" });
    await userEvent.click(
      within(region).getByRole("button", { name: "Take review mode off" }),
    );
    await screen.findByText(/ada \(2\)/);
    await userEvent.click(
      screen.getAllByRole("button", { name: "Take review mode off" })[0],
    );
    // Scoped to the card: the app frame carries live regions of its own, and a
    // bare role query would find whichever came first.
    await waitFor(() => {
      expect(within(region).getByRole("alert")).toHaveTextContent(
        /only one of them can be the file/,
      );
    });
  });
});
