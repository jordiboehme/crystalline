/**
 * Maintenance: what the knowledge needs next.
 *
 * The screen is a report rather than a workbench, and everything pinned here
 * follows from that. The queue arrives ranked and is drawn under the catalog's
 * own three families, so a reader sees the shape of the backlog rather than a
 * flat hundred rows. A finding names the engram it fired on and links there,
 * because the usual thing to do about one is to go and read the engram - and a
 * finding with no engram behind it says its subject in plain text instead,
 * rather than being dropped and counted. A judgment finding says what it is on
 * its face: a question for a person, never a change to apply. And an empty
 * queue is good news, so it is never dressed as a failure - the one thing on
 * this screen that must never wear an alert.
 *
 * The writes it does offer are pinned to the same rule. Looking records
 * nothing: acknowledging, un-acknowledging and deleting an orphaned attachment
 * each go out on a press and on nothing else, and what an acknowledgment
 * silences is counted under the queue whether or not anybody asks to see it.
 */

import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import { defined } from "../test/assert";
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
    acknowledged: {
      total: 0,
      by_family: { temporal: 0, structure: 0, redundancy: 0 },
    },
    guidance: "This queue changes nothing by itself.",
    truncations: [],
    ...overrides,
  };
}

/** An orphaned attachment: the subject is the file, so there is no engram. */
function orphanFinding() {
  return {
    n: 4,
    priority: 55,
    rule: "V108",
    class: "judgment",
    domain: "eng",
    permalink: "",
    title: "assets/2026/08/orphan.png",
    line: null,
    finding: "no engram references or claims this attachment",
    evidence: "12 KiB, image/png; no engram references or claims it",
    fix: "delete assets/2026/08/orphan.png or analyze it into an engram",
  };
}

/** A sweep whose structure family holds an anchorless finding as well. */
function orphanPayload() {
  const base = evolvePayload();
  return evolvePayload({
    total: 4,
    count: 4,
    families: [
      { family: "temporal", findings: 1 },
      { family: "structure", findings: 2 },
      { family: "redundancy", findings: 1 },
    ],
    queue: [...base.queue, orphanFinding()],
    actions: [
      ...base.actions,
      { rule: "V108", instruction: "Delete it, or analyze it into an engram." },
    ],
  });
}

/** One acknowledgment holding a finding out of the queue. */
function suppressedPayload() {
  return evolvePayload({
    acknowledged: {
      total: 1,
      by_family: { temporal: 0, structure: 1, redundancy: 0 },
    },
  });
}

/** The same sweep, asked for the silenced finding as well. */
function includingSuppressedPayload() {
  const base = evolvePayload();
  return evolvePayload({
    total: 4,
    count: 4,
    families: [
      { family: "temporal", findings: 1 },
      { family: "structure", findings: 2 },
      { family: "redundancy", findings: 1 },
    ],
    queue: [
      ...base.queue,
      {
        n: 4,
        priority: 45,
        rule: "V104",
        class: "mechanical",
        domain: "eng",
        permalink: "beta",
        title: "Beta",
        line: null,
        finding: "orphan",
        evidence: "no inbound or outbound reference",
        fix: "link it into the neighbourhood its tags suggest",
        acknowledged: true,
        ack_note: "a deliberate island",
      },
    ],
    acknowledged: {
      total: 1,
      by_family: { temporal: 0, structure: 1, redundancy: 0 },
    },
  });
}

/** A sweep whose acknowledgment was given for evidence that has since moved. */
function stalePayload() {
  const base = evolvePayload();
  return evolvePayload({
    queue: base.queue.map((finding) =>
      finding.rule === "V101"
        ? { ...finding, ack_stale: true, ack_note: "lineage citation, keep" }
        : finding,
    ),
  });
}

/**
 * A sweep whose result did not fit the page it was asked for.
 *
 * The engine ranks the whole result and answers a page of it, and its family
 * counts are over the whole result rather than over the page - which is the one
 * arrangement where a heading's count and the tally's count for the same word
 * are both right and different.
 */
function cappedPayload() {
  return evolvePayload({
    total: 250,
    limit: 100,
    families: [
      { family: "temporal", findings: 120 },
      { family: "structure", findings: 80 },
      { family: "redundancy", findings: 50 },
    ],
  });
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

/** The JSON body a write went out with. */
function sentBody(init: RequestInit | undefined): unknown {
  const body = init?.body;
  if (typeof body !== "string") {
    throw new Error("expected the write to carry a JSON body");
  }
  return JSON.parse(body) as unknown;
}

/** The disclosure panel a "How to work this" button controls. */
function panelOf(disclose: HTMLElement): HTMLElement {
  const id = disclose.getAttribute("aria-controls");
  const panel = id === null ? null : document.getElementById(id);
  if (panel === null) {
    throw new Error("expected the disclosure to control a panel");
  }
  return panel;
}

/** The finding row a piece of its text sits in. */
function rowOf(text: HTMLElement): HTMLElement {
  const row = text.closest("li");
  if (row === null) {
    throw new Error("expected that text to sit inside a finding row");
  }
  return row;
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

    // Each chip says what it is a value of, for a reader who hears the row
    // rather than seeing three coloured blocks in a line.
    expect(row).toHaveTextContent("Priority 90");
    expect(row).toHaveTextContent("Rule V005");
    expect(row).toHaveTextContent("Finding class mechanical");
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
    // A sweep that sends no summary gets no heading line invented for it: the
    // panel is the instruction and nothing else.
    expect(panelOf(disclose).textContent).toBe(
      "Complete the retirement in both halves.",
    );
  });

  it("heads the instruction with the catalog's own summary of the rule", async () => {
    // The summary is the rule in four words and the instruction is the
    // paragraph under it. Both come off the wire, so neither is derived from
    // the other and the heading is the catalog's wording rather than a
    // truncation of the body.
    await open({
      "/evolve": () =>
        evolvePayload({
          actions: [
            {
              rule: "V005",
              summary: "supersedes target still current",
              instruction: "Complete the retirement in both halves.",
            },
          ],
        }),
    });

    const temporal = await section(/^Temporal/);
    const disclose = within(temporal).getByRole("button", {
      name: /how to work this/i,
    });
    // Folded away, the heading is as absent as the paragraph under it. The
    // catalog says the rule in the same words the row's finding line does, so
    // what is counted here is the row's one copy of it, not zero.
    expect(
      within(temporal).getAllByText("supersedes target still current"),
    ).toHaveLength(1);

    await userEvent.click(disclose);

    const panel = panelOf(disclose);
    expect(
      within(panel).getByText("supersedes target still current"),
    ).toBeVisible();
    expect(
      within(panel).getByText("Complete the retirement in both halves."),
    ).toBeVisible();
  });

  it("says the engine's guidance once, above the queue", async () => {
    await open();
    await section(/^Temporal/);

    // The legend the class chips are shorthand for. It belongs to the sweep
    // rather than to a row, so it is said once rather than on every finding -
    // `getByText` is the assertion that it is exactly once.
    expect(
      screen.getByText("This queue changes nothing by itself."),
    ).toBeVisible();
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

  it("counts what is drawn, and names the whole queue as the whole queue", async () => {
    await open();

    // Nothing capped and nothing filtered: the page IS the queue, so the
    // family counts need no qualifier - they are the counts above the rows.
    expect(
      await screen.findByText(
        "42 engrams swept, 3 findings. Temporal 1, Structure 1, Redundancy 1.",
      ),
    ).toBeVisible();
  });

  it("says a capped page is a page, and what the whole of it holds", async () => {
    await open({ "/evolve": cappedPayload });

    // The heading over the temporal rows counts one finding; the breakdown
    // counts 120. Both are right, and the prefix is what keeps the same word
    // carrying two numbers from reading as a contradiction.
    expect(
      await screen.findByText(
        "42 engrams swept, 3 of 250 findings. Everything waiting: Temporal 120, Structure 80, Redundancy 50.",
      ),
    ).toBeVisible();
    expect(
      (await screen.findByRole("heading", { name: /^Temporal/ })).textContent,
    ).toMatch(/1 finding/);
  });

  it("names the base a filtered count is counted against", async () => {
    await open();
    await section(/^Temporal/);

    await userEvent.selectOptions(domainFilter(), "ops");

    // "1 finding in ops" alone would lose the only not-all-of-it signal on
    // the screen, so the page it was filtered out of is named beside it.
    expect(
      await screen.findByText(
        "42 engrams swept, 3 findings, 1 of them in ops. Everything waiting: Temporal 1, Structure 1, Redundancy 1.",
      ),
    ).toBeVisible();
  });

  it("does not sweep again when the window comes back", async () => {
    await open();
    await section(/^Temporal/);
    const before = sweeps().length;

    // The sweep is the heaviest read this API has, and this screen is a
    // snapshot with a Refresh button on it: alt-tabbing back to a page left
    // open is not somebody asking for it again.
    window.dispatchEvent(new Event("visibilitychange"));
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("focus"));
    // Flushed rather than merely awaited: a refetch that should not have
    // started has to be given every chance to reach the stub, or the count
    // below passes by being taken too early.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

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
 * The queue for somebody who is not looking at it.
 *
 * A page of a hundred rows drawn from one component is where identical
 * accessible names come from, and "How to work this" said thirty times is a
 * list of buttons nobody can choose between. Each disclosure is named by the
 * finding it belongs to, it says which panel it opens, and pressing the one
 * control that reloads the page leaves the keyboard where it was.
 */
describe("working the queue from the keyboard", () => {
  it("names each disclosure by the finding it belongs to", async () => {
    await open();
    await section(/^Temporal/);

    const body = await screen.findByRole("main");
    const names = within(body)
      .getAllByRole("button", { name: /how to work this/i })
      .map((button) => button.getAttribute("aria-label"));

    // The rule and its subject, in the families' own order: three names, all
    // different, each one enough to choose by.
    expect(names).toEqual([
      "How to work this: V005 on The old way",
      "How to work this: V101 on Alpha",
      "How to work this: V201 on Restarting the daemon",
    ]);
  });

  it("points the disclosure at the panel it opens", async () => {
    await open();

    const temporal = await section(/^Temporal/);
    const disclose = within(temporal).getByRole("button", {
      name: /how to work this/i,
    });
    const panelId = disclose.getAttribute("aria-controls");
    expect(panelId).not.toBeNull();

    await userEvent.click(disclose);

    // Everything the disclosure opened sits inside the element it names, the
    // heading line included, so following the control lands on the whole
    // panel rather than on the paragraph at the bottom of it.
    await within(temporal).findByText(
      "Complete the retirement in both halves.",
    );
    expect(panelOf(disclose)).toContainElement(
      within(temporal).getByText("Complete the retirement in both halves."),
    );
  });

  it("keeps Refresh under the keyboard while the sweep it asked for runs", async () => {
    // The second sweep is held open, so the state under test is a state the
    // test is standing in rather than one it hopes to catch.
    let land = () => {};
    const landed = new Promise<void>((resolve) => {
      land = resolve;
    });
    let holding = false;
    await open({
      "/evolve": async () => {
        if (holding) {
          await landed;
        }
        return evolvePayload();
      },
    });
    await section(/^Temporal/);
    const refresh = screen.getByRole("button", { name: "Refresh" });
    const before = sweeps().length;
    holding = true;

    await userEvent.click(refresh);

    // It used to disable itself for the length of the sweep, which in a
    // browser takes the focus off it and leaves the keyboard on the document -
    // and this is the only control in the header, so there is nothing next to
    // land on. It says it is busy instead and stays where it was.
    await waitFor(() => {
      expect(sweeps().length).toBeGreaterThan(before);
    });
    expect(refresh).toBeEnabled();
    expect(refresh).toHaveAttribute("aria-busy", "true");
    expect(refresh).toHaveFocus();

    land();

    await waitFor(() => {
      expect(refresh).toHaveAttribute("aria-busy", "false");
    });
    expect(refresh).toHaveFocus();
  });
});

/**
 * A finding that names no engram.
 *
 * Several rules are about a domain rather than about any one engram - an
 * orphaned attachment, a drifted tag vocabulary - and they arrive with an
 * empty permalink. They used to be counted and never drawn, which is a queue
 * that says four and shows three. The subject is still shown, as the plain
 * text it is: a link to an engram that does not exist would be the very thing
 * the finding is about.
 */
describe("a finding with no engram behind it", () => {
  it("names its subject in plain text rather than as a link", async () => {
    await open({ "/evolve": orphanPayload });

    const structure = await section(/^Structure/);
    const [, orphan] = rows(structure);

    expect(orphan).toHaveTextContent("assets/2026/08/orphan.png");
    expect(
      within(structure).queryByRole("link", {
        name: "assets/2026/08/orphan.png",
      }),
    ).toBeNull();
    // Counted and drawn: the tally and the rows agree.
    expect(
      await screen.findByText(
        "42 engrams swept, 4 findings. Temporal 1, Structure 2, Redundancy 1.",
      ),
    ).toBeVisible();
  });

  it("offers no acknowledgment, because there is nowhere to record one", async () => {
    await open({ "/evolve": orphanPayload });

    const [, orphan] = rows(await section(/^Structure/));

    // An acknowledgment lives in an engram's frontmatter. This finding has no
    // engram, so the action would be a button with nowhere to write.
    expect(
      within(defined(orphan, "the orphan row")).queryByRole("button", {
        name: /acknowledge/i,
      }),
    ).toBeNull();
  });

  it("deletes an orphaned attachment once the file has been named", async () => {
    const removals: (RequestInit | undefined)[] = [];
    await open({
      "/evolve": orphanPayload,
      "/domains/eng/files/assets/2026/08/orphan.png": (_path, init) => {
        removals.push(init);
        return undefined;
      },
    });
    const before = sweeps().length;
    const orphan = defined(
      rows(await section(/^Structure/))[1],
      "the orphan row",
    );

    await userEvent.click(
      within(orphan).getByRole("button", { name: "Delete attachment" }),
    );

    // The confirm names the file, because deleting bytes is irreversible.
    expect(orphan).toHaveTextContent(/Delete assets\/2026\/08\/orphan\.png\?/);

    await userEvent.click(
      within(orphan).getByRole("button", { name: "Delete" }),
    );

    await waitFor(() => {
      expect(removals).toHaveLength(1);
    });
    expect(removals[0]?.method).toBe("DELETE");
    // The exact path the sweep named, read off the row's own field rather than
    // off whatever the row is drawn with.
    expect(
      requested().filter((path) => path.startsWith("/domains/eng/files/")),
    ).toEqual(["/domains/eng/files/assets/2026/08/orphan.png"]);
    // The queue it was drawn from is stale the moment the file is gone.
    await waitFor(() => {
      expect(sweeps().length).toBeGreaterThan(before);
    });
  });

  it("offers no delete for an orphan row that names no path", async () => {
    // The display title has a fallback chain - it ends at the domain name -
    // and the delete deliberately has none. A row with no path names no file,
    // so the irreversible action is simply not there rather than aimed at
    // `eng` because that is what the heading says.
    const pathless = { ...orphanFinding() };
    delete (pathless as { title?: string }).title;
    const base = evolvePayload();
    await open({
      "/evolve": () =>
        evolvePayload({
          total: 4,
          count: 4,
          families: [
            { family: "temporal", findings: 1 },
            { family: "structure", findings: 2 },
            { family: "redundancy", findings: 1 },
          ],
          queue: [...base.queue, pathless],
        }),
    });

    const orphan = defined(
      rows(await section(/^Structure/))[1],
      "the orphan row",
    );
    expect(orphan).toHaveTextContent("eng");
    expect(
      within(orphan).queryByRole("button", { name: "Delete attachment" }),
    ).toBeNull();
  });

  it("asks before it deletes, and takes no for an answer", async () => {
    const removals: unknown[] = [];
    await open({
      "/evolve": orphanPayload,
      "/domains/eng/files/assets/2026/08/orphan.png": () => {
        removals.push(true);
        return undefined;
      },
    });
    const orphan = defined(
      rows(await section(/^Structure/))[1],
      "the orphan row",
    );

    await userEvent.click(
      within(orphan).getByRole("button", { name: "Delete attachment" }),
    );
    await userEvent.click(
      within(orphan).getByRole("button", { name: "Cancel" }),
    );

    expect(removals).toEqual([]);
    expect(
      within(orphan).getByRole("button", { name: "Delete attachment" }),
    ).toBeVisible();
  });
});

/**
 * Acknowledging: ruling a finding intentional so future sweeps stop raising
 * it, without the queue ever shrinking quietly.
 *
 * Viewing still records nothing. Every write on this screen is a button
 * somebody pressed, and what those writes silence is said out loud underneath
 * the queue with a way to look at it.
 */
describe("acknowledging a finding", () => {
  it("posts the engram, the rule and the note somebody typed, then sweeps again", async () => {
    const acks: (RequestInit | undefined)[] = [];
    await open({
      "/domains/eng/evolve/ack": (_path, init) => {
        acks.push(init);
        return undefined;
      },
    });
    const before = sweeps().length;
    const row = defined(rows(await section(/^Temporal/))[0], "the first row");

    await userEvent.click(
      within(row).getByRole("button", { name: "Acknowledge" }),
    );
    await userEvent.type(
      within(row).getByLabelText(/why is this intentional/i),
      "the lineage citation is deliberate",
    );
    await userEvent.click(
      within(row).getByRole("button", { name: "Acknowledge" }),
    );

    await waitFor(() => {
      expect(acks).toHaveLength(1);
    });
    expect(sentBody(acks[0])).toEqual({
      permalink: "notes/old-way",
      rule: "V005",
      note: "the lineage citation is deliberate",
    });
    expect(acks[0]?.method).toBe("POST");
    await waitFor(() => {
      expect(sweeps().length).toBeGreaterThan(before);
    });
  });

  it("takes an acknowledgment with no note at all", async () => {
    const acks: (RequestInit | undefined)[] = [];
    await open({
      "/domains/eng/evolve/ack": (_path, init) => {
        acks.push(init);
        return undefined;
      },
    });
    const row = defined(rows(await section(/^Temporal/))[0], "the first row");

    await userEvent.click(
      within(row).getByRole("button", { name: "Acknowledge" }),
    );
    await userEvent.click(
      within(row).getByRole("button", { name: "Acknowledge" }),
    );

    await waitFor(() => {
      expect(acks).toHaveLength(1);
    });
    expect(sentBody(acks[0])).toEqual({
      permalink: "notes/old-way",
      rule: "V005",
    });
  });

  it("says the server's own words when an acknowledgment is refused", async () => {
    await open({
      "/domains/eng/evolve/ack": () => {
        throw new ApiProblem(403, "forbidden", "this instance is read only");
      },
    });
    const row = defined(rows(await section(/^Temporal/))[0], "the first row");

    await userEvent.click(
      within(row).getByRole("button", { name: "Acknowledge" }),
    );
    await userEvent.click(
      within(row).getByRole("button", { name: "Acknowledge" }),
    );

    expect(await within(row).findByRole("alert")).toHaveTextContent(
      "this instance is read only",
    );
  });

  it("offers a reader nothing to press", async () => {
    await open({
      "/auth/me": () => meResponse({ user: userFixture({ role: "viewer" }) }),
    });
    await section(/^Temporal/);

    // The server would refuse it, and a button that only ever fails is worse
    // than no button.
    expect(screen.queryByRole("button", { name: /acknowledge/i })).toBeNull();
  });
});

/**
 * What the acknowledgments silenced: a number under the queue, and a way to
 * look at what it stands for.
 */
describe("the acknowledged findings", () => {
  it("says nothing at all when nothing is silenced", async () => {
    await open();
    await section(/^Temporal/);

    expect(screen.queryByText(/staying quiet/i)).toBeNull();
  });

  it("counts what is staying quiet, and shows it on request", async () => {
    await open({
      "/evolve": (path) =>
        path.includes("include_acknowledged=true")
          ? includingSuppressedPayload()
          : suppressedPayload(),
    });

    expect(
      await screen.findByText("1 acknowledged finding is staying quiet."),
    ).toBeVisible();
    // Nothing is fetched to say it: the count rides the ordinary sweep.
    expect(sweeps().some((path) => path.includes("include_acknowledged"))).toBe(
      false,
    );

    await userEvent.click(screen.getByRole("button", { name: "Show them" }));

    await waitFor(() => {
      expect(
        sweeps().some((path) => path.includes("include_acknowledged=true")),
      ).toBe(true);
    });
    const structure = await section(/^Structure/);
    const row = rowOf(await within(structure).findByText("Beta"));
    expect(row).toHaveTextContent("a deliberate island");
    expect(
      within(row).getByRole("button", { name: "Unacknowledge" }),
    ).toBeVisible();
    // The tally counts what is drawn, and everything is drawn: no prefix.
    expect(
      await screen.findByText(
        "42 engrams swept, 4 findings. Temporal 1, Structure 2, Redundancy 1.",
      ),
    ).toBeVisible();
  });

  it("takes an acknowledgment back and sweeps again", async () => {
    const removals: (RequestInit | undefined)[] = [];
    await open({
      "/evolve": (path) =>
        path.includes("include_acknowledged=true")
          ? includingSuppressedPayload()
          : suppressedPayload(),
      "/domains/eng/evolve/ack": (_path, init) => {
        removals.push(init);
        return undefined;
      },
    });
    await userEvent.click(
      await screen.findByRole("button", { name: "Show them" }),
    );
    const row = rowOf(await screen.findByText("Beta"));
    const before = sweeps().length;

    await userEvent.click(
      within(row).getByRole("button", { name: "Unacknowledge" }),
    );

    await waitFor(() => {
      expect(removals).toHaveLength(1);
    });
    expect(removals[0]?.method).toBe("DELETE");
    expect(sentBody(removals[0])).toEqual({
      permalink: "beta",
      rule: "V104",
    });
    await waitFor(() => {
      expect(sweeps().length).toBeGreaterThan(before);
    });
  });
});

/**
 * An acknowledgment given for evidence that has since changed.
 *
 * It is neither silenced nor forgotten: the finding comes back saying that
 * somebody ruled it intentional and that the thing they ruled on has moved.
 */
describe("an acknowledgment that no longer matches", () => {
  it("says so on the finding, with the note it was given with", async () => {
    await open({ "/evolve": stalePayload });

    const row = defined(rows(await section(/^Structure/))[0], "the stale row");

    expect(row).toHaveTextContent(
      /acknowledged earlier, but the evidence changed/i,
    );
    expect(row).toHaveTextContent("lineage citation, keep");
  });

  it("offers re-acknowledging as the action, which is the same write", async () => {
    const acks: (RequestInit | undefined)[] = [];
    await open({
      "/evolve": stalePayload,
      "/domains/eng/evolve/ack": (_path, init) => {
        acks.push(init);
        return undefined;
      },
    });
    const row = defined(rows(await section(/^Structure/))[0], "the stale row");

    await userEvent.click(
      within(row).getByRole("button", { name: "Re-acknowledge" }),
    );
    // The note it was given with is the note to keep or to correct, so it is
    // offered rather than asked for again.
    expect(within(row).getByLabelText(/why is this intentional/i)).toHaveValue(
      "lineage citation, keep",
    );
    await userEvent.click(
      within(row).getByRole("button", { name: "Re-acknowledge" }),
    );

    await waitFor(() => {
      expect(acks).toHaveLength(1);
    });
    expect(acks[0]?.method).toBe("POST");
    expect(sentBody(acks[0])).toEqual({
      permalink: "alpha",
      rule: "V101",
      note: "lineage citation, keep",
    });
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
