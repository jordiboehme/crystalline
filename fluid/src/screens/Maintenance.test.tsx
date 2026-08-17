/**
 * Maintenance: what the knowledge needs next, read-only.
 *
 * The screen is a report rather than a workbench, and everything pinned here
 * follows from that. The queue arrives ranked and is drawn under the catalog's
 * own three families, so a reader sees the shape of the backlog rather than a
 * flat hundred rows. Every finding names the engram it fired on and links
 * there, because the only thing to do about a finding is to go and read the
 * engram. A judgment finding says so on its face: it is a question for a
 * person, never a change to apply. And an empty queue is good news, so it is
 * never dressed as a failure - the one thing on this screen that must never
 * wear an alert.
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

/** One sweep with a finding in each family, in the engine's own shape. */
function evolvePayload(overrides: Record<string, unknown> = {}) {
  return {
    scope: {
      domains: ["eng", "ops"],
      families: [],
      rules: [],
      min_priority: null,
      today: "2026-08-17",
    },
    engrams_scanned: 42,
    unparsed: 0,
    total: 3,
    page: 1,
    limit: 100,
    count: 3,
    families: [
      { family: "temporal", findings: 1 },
      { family: "structure", findings: 1 },
      { family: "redundancy", findings: 1 },
    ],
    queue: [
      {
        n: 1,
        priority: 90,
        rule: "V005",
        class: "mechanical",
        domain: "eng",
        permalink: "notes/old-way",
        title: "The old way",
        line: null,
        finding: "supersedes target still current",
        evidence: "supersedes eng/new-way; new-way is stable",
        fix: "retire the old engram and wire superseded_by",
      },
      {
        n: 2,
        priority: 80,
        rule: "V201",
        class: "judgment",
        domain: "ops",
        permalink: "runbooks/restart",
        title: "Restarting the daemon",
        line: 12,
        finding: "near-duplicate content",
        evidence: "0.91 overlap with ops/runbooks/restart-service",
        fix: "merge into the richer engram and supersede the other",
      },
      {
        n: 3,
        priority: 55,
        rule: "V101",
        class: "mechanical",
        domain: "eng",
        permalink: "alpha",
        title: "Alpha",
        line: 7,
        finding: "live reference to retired",
        evidence: "links_to eng/old-way, which is deprecated",
        fix: "repoint at the successor named in the evidence",
      },
    ],
    actions: [
      { rule: "V005", instruction: "Complete the retirement in both halves." },
      { rule: "V101", instruction: "Repoint it at the successor." },
      { rule: "V201", instruction: "Merge into the richest one." },
    ],
    guidance: "This queue changes nothing by itself.",
    truncations: [],
    ...overrides,
  };
}

/** A sweep that found nothing at all. */
function cleanPayload() {
  return evolvePayload({
    total: 0,
    count: 0,
    families: [],
    queue: [],
    actions: [],
  });
}

/** The app, signed in, with whatever this test wants `/evolve` to answer. */
function serve(routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
      "/evolve": () => evolvePayload(),
      ...routes,
    }),
  );
}

/** Serve the sweep, open the screen and wait for the queue to land. */
async function open(routes: Record<string, Answer> = {}): Promise<void> {
  serve(routes);
  renderApp("/maintenance");
  await screen.findByRole("heading", {
    name: "Maintenance - what the knowledge needs next",
  });
}

/** Every path the app asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => call[0]);
}

/** Just the sweeps. */
function sweeps(): string[] {
  return requested().filter((path) => path.startsWith("/evolve"));
}

/** One family's section, once it has been drawn. */
function section(name: RegExp): Promise<HTMLElement> {
  return screen.findByRole("region", { name });
}

/** The findings of a section. */
function rows(region: HTMLElement): HTMLElement[] {
  return within(region).getAllByRole("listitem");
}

/** The domain filter. */
function domainFilter(): HTMLSelectElement {
  return screen.getByLabelText<HTMLSelectElement>("Domain");
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the maintenance screen", () => {
  it("groups the queue by family, in the catalog's own order", async () => {
    await open();

    // Scoped to the screen: the frame's sidebar heads its domain listing with
    // a second-level heading of its own.
    const body = await screen.findByRole("main");
    const headings = (
      await within(body).findAllByRole("heading", { level: 2 })
    ).map((heading) => heading.textContent ?? "");
    expect(headings).toHaveLength(3);
    expect(headings[0]).toMatch(/^Temporal/);
    expect(headings[1]).toMatch(/^Structure/);
    expect(headings[2]).toMatch(/^Redundancy/);
    // One finding under each, which is what the ranked queue held.
    expect(rows(await section(/^Temporal/))).toHaveLength(1);
    expect(rows(await section(/^Structure/))).toHaveLength(1);
    expect(rows(await section(/^Redundancy/))).toHaveLength(1);
  });

  it("draws a priority badge and a class chip on every finding", async () => {
    await open();

    const [row] = rows(await section(/^Temporal/));

    expect(row).toHaveTextContent("Priority 90");
    expect(row).toHaveTextContent("V005");
    expect(row).toHaveTextContent("mechanical");
    expect(row).toHaveTextContent("supersedes target still current");
    // The judgment finding says what it is, on its face: it is a question for
    // a person rather than a change to apply.
    const [judgment] = rows(await section(/^Redundancy/));
    expect(judgment).toHaveTextContent("judgment");
  });

  it("links a finding to the engram it fired on", async () => {
    await open();

    const temporal = await section(/^Temporal/);
    const link = within(temporal).getByRole("link", { name: "The old way" });

    // Built the way every link to an engram is built, so a multi-segment
    // permalink keeps its slashes rather than becoming one escaped segment.
    expect(link).toHaveAttribute("href", "/d/eng/e/notes/old-way");
  });

  it("offers the per-rule instruction where the rule fired", async () => {
    await open();

    const temporal = await section(/^Temporal/);
    const disclose = within(temporal).getByRole("button", {
      name: /how to work this/i,
    });
    expect(disclose).toHaveAttribute("aria-expanded", "false");
    expect(
      within(temporal).queryByText("Complete the retirement in both halves."),
    ).toBeNull();

    await userEvent.click(disclose);

    expect(
      await within(temporal).findByText(
        "Complete the retirement in both halves.",
      ),
    ).toBeVisible();
    expect(disclose).toHaveAttribute("aria-expanded", "true");
  });

  it("narrows the queue to one domain without asking the server again", async () => {
    await open();
    await section(/^Temporal/);
    const before = sweeps().length;

    await userEvent.selectOptions(domainFilter(), "ops");

    // Only the ops finding is left, and the families holding nothing for it
    // are gone rather than drawn empty.
    const redundancy = await section(/^Redundancy/);
    expect(rows(redundancy)).toHaveLength(1);
    expect(rows(redundancy)[0]).toHaveTextContent("Restarting the daemon");
    expect(screen.queryByText("The old way")).toBeNull();
    expect(screen.queryByRole("region", { name: /^Temporal/ })).toBeNull();
    // The select is fed from the whole sweep, so every domain it found is
    // still on offer after one of them narrows it.
    expect(within(domainFilter()).getAllByRole("option")).toHaveLength(3);
    // Narrowing is a lens over what already arrived, not a second sweep.
    expect(sweeps()).toHaveLength(before);
  });

  it("says an empty queue is good news rather than a failure", async () => {
    await open({ "/evolve": cleanPayload });

    expect(await screen.findByText(/nothing is waiting/i)).toBeVisible();
    // The one thing on this screen that must never wear an alert.
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("sweeps again when asked to", async () => {
    await open();
    await section(/^Temporal/);
    const before = sweeps().length;

    await userEvent.click(screen.getByRole("button", { name: "Refresh" }));

    await waitFor(() => {
      expect(sweeps().length).toBeGreaterThan(before);
    });
  });

  it("names any cap that fired, quietly, under the queue", async () => {
    await open({
      "/evolve": () =>
        evolvePayload({ truncations: ["eng - findings capped at 200"] }),
    });

    expect(
      await screen.findByText(/eng - findings capped at 200/),
    ).toBeVisible();
  });

  it("says the server's own words when the sweep fails", async () => {
    await open({
      "/evolve": () => {
        throw new ApiProblem(403, "forbidden", "this account may not sweep");
      },
    });

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "this account may not sweep",
    );
  });
});

/**
 * The way in. A read-only report about the whole instance belongs in the frame
 * beside the other whole-instance screens, and it is offered to every role
 * because there is nothing here to refuse.
 */
describe("the way to the maintenance screen", () => {
  it("stands in the frame, for every role", async () => {
    serve({
      "/auth/me": () => meResponse({ user: userFixture({ role: "viewer" }) }),
      "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
    });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    const link = screen.getByRole("link", { name: "Maintenance" });
    expect(link).toHaveAttribute("href", "/maintenance");

    await userEvent.click(link);

    expect(
      await screen.findByRole("heading", {
        name: "Maintenance - what the knowledge needs next",
      }),
    ).toBeVisible();
  });
});
