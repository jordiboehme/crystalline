import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import {
  answersFor,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";

vi.setConfig({ testTimeout: 15000 });
vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});
const apiMock = vi.mocked(api);

function thread(id: string, waiting: string, age: number, text: string) {
  return {
    id,
    path: "guides/deploying.md",
    anchor: {
      section: "7f3a9c2e",
      quote: "Deploys run at night",
      occurrence: 1,
      state: "found",
    },
    status: "open",
    waiting_on: waiting,
    opened_at: "2026-09-30T10:00:00Z",
    age_secs: age,
    entries: [
      {
        at: "2026-09-30T10:00:00Z",
        kind: "comment",
        author: {
          name: waiting === "agent" ? "vera" : "claude-code",
          agent: waiting !== "agent",
        },
        text,
      },
    ],
  };
}

const REVIEW = {
  domain: "eng",
  waiting_for_people: [
    {
      path: "guides/deploying.md",
      title: "Deploying",
      threads: [
        thread(
          "t-00000002",
          "people",
          86400 * 2,
          "Is the batch window still 2am?",
        ),
      ],
    },
  ],
  waiting_for_the_agent: [
    {
      path: "guides/deploying.md",
      title: "Deploying",
      threads: [
        thread("t-00000001", "agent", 7200, "Still true?"),
        thread("t-00000003", "agent", 60, "Typo here."),
      ],
    },
  ],
  queue: {
    domain: "eng",
    total: 53,
    page: 1,
    limit: 50,
    count: 3,
    kinds: { feedback: 1, update: 1, review: 1 },
    queue: [
      {
        n: 1,
        kind: "feedback",
        what: "changed",
        path: "guides/deploying.md",
        section: "7f3a9c2e",
        heading: "Night deploys",
        age_secs: 86400,
        changed_by: ["vera"],
        diff: "@@ -1 +1 @@\n-<!-- r:7f3a9c2e -->\n-Deploys run at night.\n+Deploys run after the batch.\n",
      },
      {
        n: 2,
        kind: "update",
        what: "source_changed",
        path: "guides/deploying.md",
        section: "7f3a9c2e",
        heading: "Night deploys",
        age_secs: 3600,
        sources: [
          {
            permalink: "runbooks/deploy",
            title: "Deploy runbook",
            state: "changed",
          },
        ],
      },
      {
        n: 3,
        kind: "review",
        what: "thread",
        path: "guides/deploying.md",
        section: "7f3a9c2e",
        thread: "t-00000001",
        age_secs: 7200,
      },
    ],
    guidance: "Work the oldest first.",
  },
};

function serve(
  review: unknown = REVIEW,
  status: Record<string, unknown> = {},
  nodes: unknown[] = [],
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": () => ({ behavior: [], domains: [] }),
      "/reflection/domains": () => ({ domains: [] }),
      "/domains/eng/reflection": () => ({
        domain: "eng",
        enabled: true,
        state: "on",
        home: "local",
        home_url: null,
        right: "curate",
        engrams_visible: true,
        pending_engrams: [],
        ...status,
      }),
      "/domains/eng/reflection/tree": () => ({
        domain: "eng",
        pact: { path: "PACT.md", title: "PACT", exists: true, open_threads: 0 },
        nodes,
      }),
      "/domains/eng/reflection/review": () => {
        if (review instanceof Error) {
          throw review;
        }
        return review;
      },
    }),
  );
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the review screen", () => {
  it("puts open threads first, split by who they wait for, oldest first with their age", async () => {
    serve();
    renderApp("/d/eng/reflection/review");
    expect(
      await screen.findByRole("heading", { name: "Review", level: 1 }),
    ).toBeVisible();
    await screen.findByRole("region", { name: "Waiting for people" });
    const headings = within(screen.getByRole("main"))
      .getAllByRole("heading", { level: 2 })
      .map((h) => h.textContent);
    expect(headings).toEqual(["Open threads", "Reflect queue"]);
    const people = screen.getByRole("region", { name: "Waiting for people" });
    expect(
      within(people).getByText("Is the batch window still 2am?"),
    ).toBeVisible();
    expect(within(people).getByText("2 days ago")).toBeVisible();
    const agent = screen.getByRole("region", { name: "Waiting for the agent" });
    const texts = within(agent)
      .getAllByText(/Still true\?|Typo here\./)
      .map((el) => el.textContent);
    expect(texts).toEqual(["Still true?", "Typo here."]);
    expect(
      within(agent).getByRole("link", { name: "Deploying" }),
    ).toHaveAttribute("href", "/d/eng/reflection/guides/deploying.md");
  });

  it("shows the reflect queue read-only: feedback and update, no review rows, no id lines", async () => {
    serve();
    renderApp("/d/eng/reflection/review");
    const queue = await screen.findByRole("region", { name: "Reflect queue" });
    const list = within(queue).getByRole("list", {
      name: "Queue items for Deploying",
    });
    const rows = Array.from(list.children) as HTMLElement[];
    expect(rows).toHaveLength(2);
    expect(
      within(rows[0] as HTMLElement).getByText("Section edited"),
    ).toBeVisible();
    expect(
      within(rows[0] as HTMLElement).getByText("changed by vera"),
    ).toBeVisible();
    expect(queue.textContent).not.toContain("r:7f3a9c2e");
    expect(
      within(rows[1] as HTMLElement).getByRole("link", {
        name: "Deploy runbook",
      }),
    ).toHaveAttribute("href", "/d/eng/e/runbooks/deploy");
    expect(within(queue).getByText("50 more in the queue")).toBeVisible();
    expect(within(queue).queryByRole("button")).toBeNull();
  });

  it("says an empty review is good news", async () => {
    serve({
      ...REVIEW,
      waiting_for_people: [],
      waiting_for_the_agent: [],
      queue: { ...REVIEW.queue, total: 0, count: 0, queue: [] },
    });
    renderApp("/d/eng/reflection/review");
    expect(await screen.findAllByText("Nobody is waiting.")).toHaveLength(2);
    expect(screen.getByText("The queue is empty.")).toBeVisible();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("links a thread to its place on the page and draws no page link for a row with no path", async () => {
    const queue = {
      ...REVIEW.queue,
      total: 1,
      count: 1,
      queue: [
        {
          n: 1,
          kind: "update",
          what: "host_moved",
          path: "",
          section: null,
          heading: "",
          age_secs: 60,
        },
      ],
    };
    serve({ ...REVIEW, queue });
    renderApp("/d/eng/reflection/review");
    const people = await screen.findByRole("region", {
      name: "Waiting for people",
    });
    expect(
      within(people).getByRole("link", { name: "2 days ago" }),
    ).toHaveAttribute(
      "href",
      "/d/eng/reflection/guides/deploying.md#t-00000002",
    );
    const list = await screen.findByRole("list", { name: "Queue items" });
    expect(
      within(list).getByText("Domain moved to this instance"),
    ).toBeVisible();
    expect(within(list).queryByRole("link")).toBeNull();
  });

  describe("file items", () => {
    function serveItem(item: Record<string, unknown>, status = {}) {
      serve(
        {
          ...REVIEW,
          queue: { ...REVIEW.queue, total: 1, count: 1, queue: [item] },
        },
        status,
      );
    }
    async function queueList() {
      return screen.findByRole("list", { name: /^Queue items/ });
    }

    it("a missing file is named under its section", async () => {
      serveItem({
        n: 1,
        kind: "feedback",
        what: "asset_missing",
        path: "guides/deploying.md",
        section: "7f3a9c2e",
        heading: "Night deploys",
        age_secs: 60,
        assets: ["assets/guides/gone.png", "assets/guides/other.pdf"],
      });
      renderApp("/d/eng/reflection/review");
      const list = await queueList();
      expect(within(list).getByText("Missing file")).toBeVisible();
      expect(within(list).getByText("assets/guides/gone.png")).toBeVisible();
      expect(within(list).getByText("assets/guides/other.pdf")).toBeVisible();
      expect(within(list).getAllByRole("link")).toHaveLength(1);
    });

    it("an engram attachment row names the link as written", async () => {
      serveItem({
        n: 1,
        kind: "feedback",
        what: "engram_attachment",
        path: "guides/deploying.md",
        section: null,
        heading: "",
        age_secs: 60,
        targets: ["../../notes/runbook.md"],
      });
      renderApp("/d/eng/reflection/review");
      const list = await queueList();
      expect(within(list).getByText("../../notes/runbook.md")).toBeVisible();
      // No section heading, so the page link is the group's own.
      expect(within(list).queryByRole("link")).toBeNull();
      expect(
        within(screen.getByRole("region", { name: "Reflect queue" })).getByRole(
          "link",
          { name: "Deploying" },
        ),
      ).toHaveAttribute("href", "/d/eng/reflection/guides/deploying.md");
    });

    it("an engram attachment row hides its targets from a reader who cannot see engrams", async () => {
      serveItem(
        {
          n: 1,
          kind: "feedback",
          what: "engram_attachment",
          path: "guides/deploying.md",
          section: null,
          heading: "",
          age_secs: 60,
          targets: ["../../notes/runbook.md"],
        },
        { engrams_visible: false },
      );
      renderApp("/d/eng/reflection/review");
      const list = await queueList();
      expect(within(list).getByText("Links to an engram file")).toBeVisible();
      expect(within(list).queryByText("../../notes/runbook.md")).toBeNull();
    });

    it("an unreferenced file row links the file", async () => {
      serveItem({
        n: 1,
        kind: "update",
        what: "asset_unreferenced",
        path: "",
        section: null,
        heading: "",
        age_secs: 60,
        asset: "assets/guides/old.png",
      });
      renderApp("/d/eng/reflection/review");
      const list = await queueList();
      const link = within(list).getByRole("link", {
        name: "assets/guides/old.png",
      });
      expect(link).toHaveAttribute("target", "_blank");
      expect(link.getAttribute("href")).toContain(
        "/domains/eng/reflection/assets/",
      );
      expect(within(list).getAllByRole("link")).toHaveLength(1);
    });

    it("a host move row says what is only on the previous host, with no page link", async () => {
      serveItem({
        n: 4,
        kind: "update",
        what: "host_moved",
        domain: "eng",
        path: "",
        section: null,
        heading: "",
        age_secs: 12,
        only_elsewhere: {
          previous_host: "alpha",
          engrams: 1,
          attachments: 0,
          pages: 1,
          files: 1,
        },
      });
      renderApp("/d/eng/reflection/review");
      const list = await queueList();
      expect(
        within(list).getByText(
          "This instance took over the domain. Only on alpha: 1 engram, 1 page and 1 file.",
        ),
      ).toBeVisible();
      expect(
        within(list).getByText(
          "Copy the domain folder over and sync. If the files are gone for good, run crystalline sync --domain eng --accept-disk.",
        ),
      ).toBeVisible();
      expect(within(list).queryByRole("link")).toBeNull();
    });

    it("a host move row with no previous host says the previous instance", async () => {
      serveItem({
        n: 4,
        kind: "update",
        what: "host_moved",
        path: "",
        section: null,
        heading: "",
        age_secs: 12,
        only_elsewhere: { previous_host: null, attachments: 2 },
      });
      renderApp("/d/eng/reflection/review");
      const list = await queueList();
      expect(
        within(list).getByText(
          "This instance took over the domain. Only on the previous instance: 2 attachments.",
        ),
      ).toBeVisible();
    });
  });

  it("names a section that cites a domain fewer people may read, with its source", async () => {
    serve({
      ...REVIEW,
      queue: {
        ...REVIEW.queue,
        queue: [
          {
            n: 1,
            kind: "update",
            what: "source_too_wide",
            path: "guides/deploying.md",
            section: "7f3a9c2e",
            heading: "Night deploys",
            age_secs: 3600,
            sources: [
              {
                permalink: "team/harbor-team",
                title: "Harbor team",
                state: "current",
                domain: "harbor",
              },
            ],
            note: "this section cites a domain fewer people may read than this page's domain",
          },
        ],
      },
    });
    renderApp("/d/eng/reflection/review");
    const queue = await screen.findByRole("region", { name: "Reflect queue" });
    expect(
      await within(queue).findByText("Cites a domain fewer people read"),
    ).toBeVisible();
    expect(
      within(queue).getByText(
        /This section cites a domain fewer people may read than this one\./,
      ),
    ).toBeVisible();
    expect(
      within(queue).getByRole("link", { name: "Harbor team in domain harbor" }),
    ).toHaveAttribute("href", "/d/harbor/e/team/harbor-team");
  });

  it("asks the person before it shares a too wide section on purpose", async () => {
    const posted: unknown[] = [];
    const question =
      "Share text from harbor on a page more people may read? The page guides/deploying.md in eng is read by everyone who can open this instance.";
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture() }),
        "/domains": () => ({ behavior: [], domains: [] }),
        "/reflection/domains": () => ({ domains: [] }),
        "/domains/eng/reflection": () => ({
          domain: "eng",
          enabled: true,
          state: "on",
          home: "local",
          home_url: null,
          right: "curate",
          engrams_visible: true,
          pending_engrams: [],
        }),
        "/domains/eng/reflection/tree": () => ({
          domain: "eng",
          pact: {
            path: "PACT.md",
            title: "PACT",
            exists: true,
            open_threads: 0,
          },
          nodes: [],
        }),
        "/domains/eng/reflection/review": () => ({
          ...REVIEW,
          queue: {
            ...REVIEW.queue,
            queue: [
              {
                n: 1,
                kind: "update",
                what: "source_too_wide",
                path: "guides/deploying.md",
                section: "7f3a9c2e",
                heading: "Night deploys",
                age_secs: 3600,
                sources: [],
                question,
                basis: "b1",
              },
            ],
          },
        }),
        "/domains/eng/reflection/disclose": (_path, init) => {
          posted.push(JSON.parse(init?.body as string));
          return {
            domain: "eng",
            path: "guides/deploying.md",
            section: "7f3a9c2e",
            disclosed: { by: "jordi", at: "2026-10-28T09:00:00Z" },
          };
        },
      }),
    );
    const user = userEvent.setup();
    renderApp("/d/eng/reflection/review");
    const queue = await screen.findByRole("region", { name: "Reflect queue" });
    await user.click(
      await within(queue).findByRole("button", { name: "Share on purpose" }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "Share text from a domain fewer people read",
    });
    expect(within(dialog).getByText(question)).toBeVisible();
    // Cancel shares nothing.
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(posted).toEqual([]);
    expect(screen.queryByRole("dialog")).toBeNull();
    await user.click(
      within(queue).getByRole("button", { name: "Share on purpose" }),
    );
    const again = await screen.findByRole("dialog", {
      name: "Share text from a domain fewer people read",
    });
    await user.click(within(again).getByRole("button", { name: "Share it" }));
    await waitFor(() => {
      expect(posted).toEqual([
        { path: "guides/deploying.md", section: "7f3a9c2e", basis: "b1" },
      ]);
    });
  });

  it("says so and reads the review again when the readers changed since the question", async () => {
    let reviews = 0;
    const changed =
      "who may read this page, or where it is backed up, changed since the person was asked; ask them again";
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () => meResponse({ user: userFixture() }),
        "/domains": () => ({ behavior: [], domains: [] }),
        "/reflection/domains": () => ({ domains: [] }),
        "/domains/eng/reflection": () => ({
          domain: "eng",
          enabled: true,
          state: "on",
          home: "local",
          home_url: null,
          right: "curate",
          engrams_visible: true,
          pending_engrams: [],
        }),
        "/domains/eng/reflection/tree": () => ({
          domain: "eng",
          pact: {
            path: "PACT.md",
            title: "PACT",
            exists: true,
            open_threads: 0,
          },
          nodes: [],
        }),
        "/domains/eng/reflection/review": () => {
          reviews += 1;
          return {
            ...REVIEW,
            queue: {
              ...REVIEW.queue,
              queue: [
                {
                  n: 1,
                  kind: "update",
                  what: "source_too_wide",
                  path: "guides/deploying.md",
                  section: "7f3a9c2e",
                  heading: "Night deploys",
                  age_secs: 3600,
                  sources: [],
                  question: "Share text from harbor?",
                  basis: "b1",
                },
              ],
            },
          };
        },
        "/domains/eng/reflection/disclose": () => {
          throw new ApiProblem(409, "Conflict", changed);
        },
      }),
    );
    const user = userEvent.setup();
    renderApp("/d/eng/reflection/review");
    const queue = await screen.findByRole("region", { name: "Reflect queue" });
    await user.click(
      await within(queue).findByRole("button", { name: "Share on purpose" }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "Share text from a domain fewer people read",
    });
    const before = reviews;
    await user.click(within(dialog).getByRole("button", { name: "Share it" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(changed);
    await waitFor(() => {
      expect(reviews).toBeGreaterThan(before);
    });
  });

  it("offers no confirmation for a too wide item without a question", async () => {
    serve({
      ...REVIEW,
      queue: {
        ...REVIEW.queue,
        queue: [
          {
            n: 1,
            kind: "update",
            what: "source_too_wide",
            path: "guides/deploying.md",
            section: "7f3a9c2e",
            heading: "Night deploys",
            age_secs: 3600,
            sources: [],
          },
        ],
      },
    });
    renderApp("/d/eng/reflection/review");
    const queue = await screen.findByRole("region", { name: "Reflect queue" });
    await within(queue).findByText("Cites a domain fewer people read");
    expect(
      within(queue).queryByRole("button", { name: "Share on purpose" }),
    ).toBeNull();
  });

  it("links a source of another domain into its domain and counts the ones the reader cannot open", async () => {
    serve({
      ...REVIEW,
      queue: {
        ...REVIEW.queue,
        queue: [
          {
            n: 1,
            kind: "update",
            what: "source_changed",
            path: "guides/deploying.md",
            section: "7f3a9c2e",
            heading: "Night deploys",
            age_secs: 3600,
            sources: [
              {
                permalink: "team/harbor-team",
                title: "Harbor team",
                state: "changed",
                domain: "harbor",
              },
            ],
            hidden_sources: 1,
            note: "a source in a domain you cannot open has changed",
          },
          {
            n: 2,
            kind: "update",
            what: "source_unavailable",
            path: "guides/deploying.md",
            section: null,
            heading: "",
            age_secs: 3600,
            source_domain: "elsewhere",
            count: 2,
            note: "sources in elsewhere cannot be checked here",
          },
        ],
      },
    });
    renderApp("/d/eng/reflection/review");
    const queue = await screen.findByRole("region", { name: "Reflect queue" });
    // The link's name carries the domain, so two titles in two domains differ.
    expect(
      await within(queue).findByRole("link", {
        name: "Harbor team in domain harbor",
      }),
    ).toHaveAttribute("href", "/d/harbor/e/team/harbor-team");
    expect(
      within(queue).getByText("1 source in a domain you cannot open"),
    ).toBeVisible();
    expect(within(queue).getByText("Sources not checked here")).toBeVisible();
    expect(
      within(queue).getByText("2 sources in elsewhere cannot be checked here."),
    ).toBeVisible();
  });

  it("draws no source engram links or titles for a reader who cannot see engrams", async () => {
    serve(REVIEW, { engrams_visible: false });
    renderApp("/d/eng/reflection/review");
    const queue = await screen.findByRole("region", { name: "Reflect queue" });
    const list = await within(queue).findByRole("list", {
      name: /^Queue items/,
    });
    expect(within(list).getByText("A source changed")).toBeVisible();
    expect(
      within(list).queryByRole("link", { name: "Deploy runbook" }),
    ).toBeNull();
    expect(queue.textContent).not.toContain("Deploy runbook");
  });

  describe("the queue, grouped by page", () => {
    const DIFF = [
      "--- original",
      "+++ modified",
      "@@ -1,12 +1,12 @@",
      " line one",
      " line two",
      " line three",
      " line four",
      " line five",
      "-old sentence",
      "+new sentence",
      " line six",
      " line seven",
      " line eight",
      " line nine",
      " line ten",
      "\\ No newline at end of file",
      "",
    ].join("\n");
    function queueOf(items: Record<string, unknown>[]) {
      return {
        ...REVIEW,
        waiting_for_people: [],
        waiting_for_the_agent: [],
        queue: {
          ...REVIEW.queue,
          total: items.length,
          count: items.length,
          queue: items,
        },
      };
    }
    const row = (n: number, path: string, extra = {}) => ({
      n,
      kind: "feedback",
      what: "changed",
      path,
      section: "7f3a9c2e",
      heading: "Night deploys",
      age_secs: 60,
      ...extra,
    });

    it("groups the items under one link per page, titled from the tree", async () => {
      serve(
        queueOf([
          row(1, "guides/deploying.md"),
          row(2, "ops/failover.md"),
          row(3, "guides/deploying.md", { what: "asset_missing" }),
          {
            n: 4,
            kind: "update",
            what: "asset_unreferenced",
            path: "",
            section: null,
            heading: "",
            age_secs: 60,
            asset: "assets/old.png",
          },
        ]),
        {},
        [
          {
            kind: "reflection",
            name: "deploying.md",
            path: "guides/deploying.md",
            title: "Deploying",
            open_threads: 0,
            sections: 2,
          },
        ],
      );
      renderApp("/d/eng/reflection/review");
      const queue = await screen.findByRole("region", {
        name: "Reflect queue",
      });
      const groups = await within(queue).findByRole("list", {
        name: "Pages in the queue",
      });
      const heads = within(groups)
        .getAllByRole("heading", { level: 3 })
        .map((h) => h.textContent);
      // The path with no page comes last; a page the tree lacks shows its path.
      expect(heads).toEqual(["Deploying", "ops/failover.md", "Not on a page"]);
      const first = within(groups).getByRole("list", {
        name: "Queue items for Deploying",
      });
      expect(first.children).toHaveLength(2);
      expect(
        within(groups).getByRole("link", { name: "ops/failover.md" }),
      ).toHaveAttribute("href", "/d/eng/reflection/ops/failover.md");
    });

    const NODES = [
      {
        kind: "reflection",
        name: "deploying.md",
        path: "guides/deploying.md",
        title: "Deploying",
        open_threads: 0,
        sections: 2,
      },
    ];
    const DIFF_NAME = "Changes in Night deploys, Deploying";
    function serveDiff(diff: string, extra = {}) {
      serve(
        queueOf([row(1, "guides/deploying.md", { diff, ...extra })]),
        {},
        NODES,
      );
    }

    it("shows the engine's hunks as they are, with no file header lines", async () => {
      serveDiff(DIFF);
      renderApp("/d/eng/reflection/review");
      const diff = await screen.findByRole("group", { name: DIFF_NAME });
      expect(diff).toHaveTextContent("new sentence");
      expect(diff).toHaveTextContent("old sentence");
      // The whole hunk, context included, and no disclosure for it.
      expect(diff).toHaveTextContent("line one");
      expect(diff).toHaveTextContent("line ten");
      expect(screen.queryByRole("button", { name: /full diff/ })).toBeNull();
      for (const hidden of [
        "--- original",
        "+++ modified",
        "No newline at end of file",
        "@@",
      ]) {
        expect(diff.textContent).not.toContain(hidden);
      }
    });

    it("announces a run of added or removed lines once", async () => {
      serveDiff("@@ -1,2 +1,3 @@\n-a\n-b\n+c\n+d\n+e\n");
      renderApp("/d/eng/reflection/review");
      const diff = await screen.findByRole("group", { name: DIFF_NAME });
      expect(diff.textContent?.match(/Removed: /g)).toHaveLength(1);
      expect(diff.textContent?.match(/Added: /g)).toHaveLength(1);
    });

    it("shows a pure addition as one block with one announcement", async () => {
      serveDiff("@@ -0,0 +1,3 @@\n+## New\n+\n+Text.\n");
      renderApp("/d/eng/reflection/review");
      const diff = await screen.findByRole("group", { name: DIFF_NAME });
      expect(diff.textContent?.match(/Added: /g)).toHaveLength(1);
      expect(diff.textContent).not.toContain("Removed");
      expect(diff).toHaveTextContent("## New");
    });

    it("shows a removal with nothing added", async () => {
      serveDiff("@@ -1,2 +0,0 @@\n-gone\n-also gone\n");
      renderApp("/d/eng/reflection/review");
      const diff = await screen.findByRole("group", { name: DIFF_NAME });
      expect(diff).toHaveTextContent("also gone");
      expect(diff.textContent).not.toContain("Added");
    });

    it("keeps a hunk that starts at the first line and separates two hunks with a gap", async () => {
      serveDiff("@@ -1 +1 @@\n-first\n+First\n@@ -20 +20 @@\n-late\n+Late\n");
      renderApp("/d/eng/reflection/review");
      const diff = await screen.findByRole("group", { name: DIFF_NAME });
      expect(diff).toHaveTextContent("First");
      expect(diff).toHaveTextContent("Late");
      expect(diff).toHaveTextContent("Unchanged lines left out");
    });

    it("reads a CRLF diff without carriage returns", async () => {
      serveDiff("@@ -1 +1 @@\r\n-old\r\n+new\r\n");
      renderApp("/d/eng/reflection/review");
      const diff = await screen.findByRole("group", { name: DIFF_NAME });
      expect(diff.textContent).not.toContain("\r");
      expect(diff).toHaveTextContent("new");
    });

    it("drops a real id line and keeps a line that only looks like one", async () => {
      serveDiff(
        "@@ -1,3 +1,3 @@\n <!-- r:7f3a9c2e -->\n-<!-- r:not-an-id -->\n+<!-- note: keep -->\n",
      );
      renderApp("/d/eng/reflection/review");
      const diff = await screen.findByRole("group", { name: DIFF_NAME });
      expect(diff.textContent).not.toContain("r:7f3a9c2e");
      expect(diff).toHaveTextContent("<!-- r:not-an-id -->");
      expect(diff).toHaveTextContent("<!-- note: keep -->");
    });

    it("keeps a long line whole inside a scrolling box", async () => {
      const long = "x".repeat(400);
      serveDiff(`@@ -1 +1 @@\n-short\n+${long}\n`);
      renderApp("/d/eng/reflection/review");
      const diff = await screen.findByRole("group", { name: DIFF_NAME });
      expect(diff).toHaveTextContent(long);
      expect(diff.className).toContain("overflow-x-auto");
    });

    it("names the diff by the page alone when the item has no heading", async () => {
      serve(
        queueOf([
          row(1, "guides/deploying.md", {
            heading: "",
            diff: "@@ -1 +1 @@\n-a\n+b\n",
          }),
        ]),
        {},
        NODES,
      );
      renderApp("/d/eng/reflection/review");
      expect(
        await screen.findByRole("group", { name: "Changes in Deploying" }),
      ).toBeVisible();
    });

    it("says in one plain sentence which folders the pact names", async () => {
      serve(
        queueOf([
          row(1, "misc/stray.md", {
            what: "off_structure",
            kind: "update",
            heading: "",
            structure: [
              "guides/` for how to do things",
              "`decisions/` for why",
              "onboarding/**",
            ],
          }),
        ]),
      );
      renderApp("/d/eng/reflection/review");
      expect(
        await screen.findByText(
          "This page is not in a folder the pact names (guides, decisions, onboarding).",
        ),
      ).toBeVisible();
    });

    it("shows the pact page as Pact", async () => {
      serve(
        queueOf([row(1, "PACT.md", { what: "pact_changed", heading: "" })]),
      );
      renderApp("/d/eng/reflection/review");
      expect(
        await screen.findByRole("heading", { name: "Pact", level: 3 }),
      ).toBeVisible();
    });

    it("says no raw words: one plain label per kind of item, unknown ones put into words", async () => {
      serve(
        queueOf([
          row(1, "guides/deploying.md", {
            what: "source_changed",
            kind: "update",
          }),
          row(2, "guides/deploying.md", {
            what: "brand_new_thing",
            kind: "update",
          }),
        ]),
      );
      renderApp("/d/eng/reflection/review");
      const queue = await screen.findByRole("region", {
        name: "Reflect queue",
      });
      expect(await within(queue).findByText("A source changed")).toBeVisible();
      expect(within(queue).getByText("Brand new thing")).toBeVisible();
      expect(queue.textContent).not.toMatch(/source_changed|feedback|update/);
    });

    it("counts unreflected engrams with the right verb for a reader of engrams", async () => {
      const item = (n: number, count: number, folder: string) => ({
        n,
        kind: "update",
        what: "unreflected",
        path: "",
        section: null,
        heading: "",
        age_secs: 60,
        count,
        folder,
        engrams: [`${folder}/a`],
      });
      serve(queueOf([item(1, 1, "notes"), item(2, 3, "runbooks")]));
      renderApp("/d/eng/reflection/review");
      expect(
        await screen.findByText("1 engram under notes has no page yet."),
      ).toBeVisible();
      expect(
        screen.getByText("3 engrams under runbooks have no page yet."),
      ).toBeVisible();
    });

    it("shows a reader without engram rights no unreflected row and no folder", async () => {
      serve(
        queueOf([
          {
            n: 1,
            kind: "update",
            what: "unreflected",
            path: "",
            section: null,
            heading: "",
            age_secs: 60,
            count: 2,
            folder: "secret-folder",
          },
          row(2, "guides/deploying.md"),
        ]),
        { engrams_visible: false },
      );
      renderApp("/d/eng/reflection/review");
      const queue = await screen.findByRole("region", {
        name: "Reflect queue",
      });
      await within(queue).findByText("Section edited");
      expect(queue.textContent).not.toContain("secret-folder");
      expect(queue.textContent).not.toContain("no page yet");
      expect(queue.textContent).not.toContain("No page yet");
    });

    it("leaks no engram names to a reader without engram rights", async () => {
      serve(
        queueOf([
          row(1, "guides/deploying.md", {
            sources: [
              {
                permalink: "runbooks/deploy",
                title: "Deploy runbook",
                state: "changed",
              },
            ],
            targets: ["../../notes/runbook.md"],
            engrams: ["runbooks/deploy"],
          }),
        ]),
        { engrams_visible: false },
      );
      renderApp("/d/eng/reflection/review");
      const queue = await screen.findByRole("region", {
        name: "Reflect queue",
      });
      await within(queue).findByRole("list", { name: /^Queue items/ });
      for (const leak of ["Deploy runbook", "runbooks/deploy", "runbook.md"]) {
        expect(queue.textContent).not.toContain(leak);
      }
    });
  });

  it("draws the open threads in the thread look: avatars, hairlines, a waiting line, no boxes", async () => {
    serve();
    renderApp("/d/eng/reflection/review");
    const agent = await screen.findByRole("region", {
      name: "Waiting for the agent",
    });
    const cards = within(agent).getAllByRole("article");
    expect(cards).toHaveLength(2);
    for (const card of cards) {
      expect(card.className).not.toContain("border");
      expect(card.querySelector("[data-avatar]")).not.toBeNull();
      expect(card).toHaveTextContent("Waiting for the agent");
      expect(card.parentElement?.className).toContain("border-t border-hair");
    }
    expect(
      within(
        screen.getByRole("region", { name: "Waiting for people" }),
      ).getByRole("article"),
    ).toHaveTextContent("Waiting for people");
  });

  it("shows the state notice instead of the review while the Reflection is off", async () => {
    serve(REVIEW, { enabled: false, state: "off" });
    renderApp("/d/eng/reflection/review");
    expect(
      await screen.findByText(/The Reflection is off for this domain/),
    ).toBeVisible();
    expect(screen.queryByRole("region", { name: "Reflect queue" })).toBeNull();
    expect(
      apiMock.mock.calls.some(([path]) =>
        String(path).endsWith("/reflection/review"),
      ),
    ).toBe(false);
  });

  it("shows the server's own sentence to a reader whose right is none", async () => {
    serve(
      new ApiProblem(403, "forbidden", "You may not read this Reflection."),
      { right: "none" },
    );
    renderApp("/d/eng/reflection/review");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "You may not read this Reflection.",
    );
    expect(screen.queryByRole("region", { name: "Reflect queue" })).toBeNull();
  });
});
