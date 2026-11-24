/**
 * A Reflection page's life beside its text (design 4): the status lines
 * under the title (the pacts that apply, the clock), the details column
 * with its read-only view of the page settings and the form behind Edit,
 * the page pact, retire from the More actions menu, restore and delete on
 * the retired banner, and the clock that decides when the text is read
 * again.
 */

import { focusManager } from "@tanstack/react-query";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { Answer } from "../test/harness";
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

const PAGE = "arch/overview/db.md";
const URL = `/d/eng/reflection/${PAGE}`;
const DOC = `/domains/eng/reflections/${PAGE}`;
const HOUR = 3600;

function view(overrides: Record<string, unknown> = {}) {
  return {
    freshness: "1d",
    follows: null,
    status: null,
    owner: null,
    retired: null,
    chain: [{ path: "arch/overview.md", title: "Architecture overview" }],
    pacts: ["PACT.md", "arch/overview.pact.md"],
    pact: { path: "arch/overview/db.pact.md", exists: false },
    findings: [],
    ...overrides,
  };
}

function doc(page: Record<string, unknown> | null = view(), checksum = "c1") {
  return {
    domain: "eng",
    path: PAGE,
    title: "Database",
    markdown: "# Database\n\nText.\n",
    checksum,
    sections: [],
    threads: [],
    ...(page === null ? {} : { page }),
  };
}

function clockOf(overrides: Record<string, unknown> = {}) {
  return {
    path: PAGE,
    freshness: "1d",
    last_reconciled: "2026-11-20T10:00:00Z",
    last_checked_secs: 2 * HOUR,
    due_in_secs: HOUR,
    stale: false,
    overdue_secs: null,
    missed_secs: null,
    loop_slower_than_freshness: false,
    waiting: false,
    part_of: null,
    clock_of: PAGE,
    ...overrides,
  };
}

function serve(
  right: string,
  routes: Record<string, Answer> = {},
  { readOnly = false }: { readOnly?: boolean } = {},
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({
          user: userFixture({ name: "ann", role: "editor" }),
          read_only: readOnly,
        }),
      "/domains": () => ({ behavior: [], domains: [] }),
      "/reflection/domains": () => ({ domains: [] }),
      "/domains/eng/reflection": () => ({
        domain: "eng",
        enabled: true,
        state: "on",
        home: "local",
        home_url: null,
        right,
        engrams_visible: true,
        pending_engrams: [],
      }),
      "/domains/eng/reflection/tree": () => ({
        domain: "eng",
        pact: { path: "PACT.md", title: "PACT", exists: true, open_threads: 0 },
        nodes: [
          {
            kind: "reflection",
            name: "overview.md",
            path: "arch/overview.md",
            title: "Architecture overview",
          },
          {
            kind: "reflection",
            name: "old.md",
            path: "arch/old.md",
            title: "Old page",
            retired: true,
          },
          { kind: "reflection", name: "db.md", path: PAGE, title: "Database" },
        ],
      }),
      [DOC]: () => doc(),
      "/domains/eng/reflection/clock": () => ({
        clock: clockOf(),
        checksum: "c1",
      }),
      "/domains/eng/reflection/assets": () => ({ assets: [] }),
      ...routes,
    }),
  );
}

/** The calls made to `path` with `method`, their bodies parsed. */
function sent(method: string, path: string): unknown[] {
  return apiMock.mock.calls
    .filter(([p, init]) => p === path && init?.method === method)
    .map(([, init]) =>
      typeof init?.body === "string"
        ? (JSON.parse(init.body) as unknown)
        : null,
    );
}

beforeEach(() => {
  apiMock.mockReset();
});

beforeAll(async () => {
  await import("./ReflectionPage");
  await import("../components/MarkdownBody");
}, 120_000);

async function details(): Promise<HTMLElement> {
  return screen.findByRole("complementary", { name: "Page details" });
}

/** Opens the settings form behind Edit and returns it. */
async function edit(panel: HTMLElement): Promise<HTMLElement> {
  await userEvent.click(
    await within(panel).findByRole("button", { name: "Edit page settings" }),
  );
  return within(panel).getByRole("form", { name: "Page settings" });
}

/** Opens the page's More actions menu. */
async function moreActions(): Promise<void> {
  await userEvent.click(
    await screen.findByRole("button", { name: "More actions" }),
  );
}

describe("the status lines under the title", () => {
  it("say the pacts that apply, each a link to its file, and the clock", async () => {
    serve("read");
    renderApp(URL);
    const status = await screen.findByTestId("page-status");
    expect(status).toHaveTextContent("Pacts: PACT.md > overview.pact.md");
    expect(
      within(status).getByRole("link", { name: "PACT.md" }),
    ).toHaveAttribute("href", "/d/eng/reflection/PACT.md");
    expect(
      within(status).getByRole("link", { name: "overview.pact.md" }),
    ).toHaveAttribute("href", "/d/eng/reflection/arch/overview.pact.md");
    expect(
      await within(status).findByText(
        "Last checked 2 hours ago, due again in 1 hour",
      ),
    ).toBeVisible();
  });

  it("stand under the header's title when the text has none of its own", async () => {
    serve("read", {
      [DOC]: () => ({ ...doc(), markdown: "Text without a title.\n" }),
    });
    renderApp(URL);
    const status = await screen.findByTestId("page-status");
    expect(status.closest("header")).not.toBeNull();
    expect(status.closest("article")).toBeNull();
    expect(status).toHaveTextContent("Pacts: PACT.md > overview.pact.md");
  });

  it("end with the page's own pact once it exists", async () => {
    serve("read", {
      [DOC]: () =>
        doc(
          view({
            pacts: [
              "PACT.md",
              "arch/overview.pact.md",
              "arch/overview/db.pact.md",
            ],
            pact: { path: "arch/overview/db.pact.md", exists: true },
          }),
        ),
    });
    renderApp(URL);
    const status = await screen.findByTestId("page-status");
    expect(status).toHaveTextContent(
      "Pacts: PACT.md > overview.pact.md > db.pact.md",
    );
    expect(
      within(status).getByRole("link", { name: "db.pact.md" }),
    ).toHaveAttribute("href", "/d/eng/reflection/arch/overview/db.pact.md");
  });

  it("say since when an overdue page is overdue, in the warning colour, with the loop hint", async () => {
    serve("read", {
      "/domains/eng/reflection/clock": () => ({
        clock: clockOf({
          stale: true,
          due_in_secs: null,
          last_checked_secs: 5 * HOUR,
          overdue_secs: 3 * HOUR,
          missed_secs: 600,
          loop_slower_than_freshness: true,
        }),
        checksum: "c1",
      }),
    });
    renderApp(URL);
    const line = await screen.findByText(
      "Overdue since 3 hours, last checked 5 hours ago",
    );
    expect(line.className).toContain("text-amber-800");
    expect(
      screen.getByText(
        "The reflect loop runs less often than this page's freshness.",
      ),
    ).toBeVisible();
    expect(screen.getByText("A reflect run skipped this page.")).toBeVisible();
  });

  it("say waiting while the page is open in the editor", async () => {
    serve("read", {
      "/domains/eng/reflection/clock": () => ({
        clock: clockOf({
          stale: true,
          due_in_secs: null,
          overdue_secs: HOUR,
          waiting: true,
        }),
        checksum: "c1",
      }),
    });
    renderApp(URL);
    expect(
      await screen.findByText("Waiting: open in the editor"),
    ).toBeVisible();
    expect(screen.queryByText(/Overdue since/)).toBeNull();
  });

  it("say part of the owner, and to a curator why an owner without freshness keeps no clock", async () => {
    const owned = view({
      freshness: null,
      owner: {
        path: "arch/overview.md",
        title: "Architecture overview",
        exists: true,
      },
    });
    const routes = {
      [DOC]: () => doc(owned),
      "/domains/eng/reflection/clock": () => ({
        clock: clockOf({
          freshness: null,
          last_reconciled: null,
          last_checked_secs: null,
          due_in_secs: null,
          clock_of: null,
          part_of: { path: "arch/overview.md", title: "Architecture overview" },
        }),
        checksum: "c1",
      }),
    };
    serve("curate", routes);
    const first = renderApp(URL);
    const status = await screen.findByTestId("page-status");
    expect(status).toHaveTextContent("Part of: Architecture overview");
    expect(
      await within(status).findByText(
        "Architecture overview has no freshness, so this page is not checked on a schedule",
      ),
    ).toBeVisible();
    first.unmount();
    serve("read", routes);
    renderApp(URL);
    const again = await screen.findByTestId("page-status");
    expect(again).toHaveTextContent("Part of: Architecture overview");
    expect(within(again).queryByText(/has no freshness/)).toBeNull();
  });
});

describe("the page details column", () => {
  it("reads the settings to a reader, with no control", async () => {
    serve("edit");
    renderApp(URL);
    const panel = await details();
    // In words, as the form says them.
    expect(within(panel).getByText("Freshness").nextSibling).toHaveTextContent(
      /^1 day$/,
    );
    expect(within(panel).getByText("No page, only PACT.md")).toBeVisible();
    expect(within(panel).getByText("Page pact").nextSibling).toHaveTextContent(
      /^None$/,
    );
    expect(within(panel).queryByRole("button")).toBeNull();
    expect(within(panel).queryByRole("combobox")).toBeNull();
    // The section is named once, by its heading.
    const section = within(panel).getByRole("region", {
      name: "Page settings",
    });
    expect(section).not.toHaveAttribute("aria-label");
    expect(section).toHaveAttribute(
      "aria-labelledby",
      within(section).getByRole("heading", { name: "Page settings" }).id,
    );
  });

  it("says a custom freshness in words too", async () => {
    for (const [written, said] of [
      ["3m", "3 months"],
      ["2w", "2 weeks"],
      ["12h", "12 hours"],
      ["1y", "1 year"],
      ["5", "5 days"],
      ["soon", "soon"],
      // The server reads units in lower case only: these are no freshness.
      ["3M", "3M"],
      ["1D", "1D"],
    ]) {
      serve("edit", { [DOC]: () => doc(view({ freshness: written })) });
      const shown = renderApp(URL);
      const panel = await details();
      expect(
        within(panel).getByText("Freshness").nextSibling,
      ).toHaveTextContent(new RegExp(`^${said}$`));
      shown.unmount();
    }
  });

  it("keeps the freshness when Custom is picked with an empty field", async () => {
    serve("curate", {
      "/domains/eng/reflection/settings": () => ({
        domain: "eng",
        path: PAGE,
        checksum: "c2",
        page: view(),
      }),
    });
    renderApp(URL);
    const panel = await edit(await details());
    await userEvent.selectOptions(
      within(panel).getByRole("combobox", { name: "Freshness" }),
      "custom",
    );
    const save = within(panel).getByRole("button", { name: "Save" });
    expect(save).toHaveAttribute("aria-disabled", "true");
    await userEvent.type(
      within(panel).getByRole("textbox", { name: "Custom freshness" }),
      "  ",
    );
    expect(save).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(save);
    expect(sent("PATCH", "/domains/eng/reflection/settings")).toEqual([]);
  });

  it("lets a curator pick a preset and sends only that", async () => {
    serve("curate", {
      "/domains/eng/reflection/settings": () => ({
        domain: "eng",
        path: PAGE,
        checksum: "c2",
        page: view({ freshness: "6h" }),
      }),
    });
    renderApp(URL);
    const panel = await edit(await details());
    const save = within(panel).getByRole("button", { name: "Save" });
    expect(save).toHaveAttribute("aria-disabled", "true");
    await userEvent.selectOptions(
      within(panel).getByRole("combobox", { name: "Freshness" }),
      "6h",
    );
    await userEvent.click(save);
    await waitFor(() => {
      expect(sent("PATCH", "/domains/eng/reflection/settings")).toEqual([
        { path: PAGE, freshness: "6h" },
      ]);
    });
  });

  it("clears a setting with an empty value", async () => {
    serve("curate", {
      [DOC]: () => doc(view({ follows: "arch/overview.md" })),
      "/domains/eng/reflection/settings": () => ({
        domain: "eng",
        path: PAGE,
        checksum: "c2",
        page: view(),
      }),
    });
    renderApp(URL);
    const panel = await edit(await details());
    const follows = within(panel).getByRole("combobox", { name: "Follows" });
    // The pages of the domain, but neither the page itself nor a retired one.
    await waitFor(() => {
      expect(
        within(follows).getByRole("option", { name: "Architecture overview" }),
      ).toBeInTheDocument();
    });
    expect(
      within(follows).queryByRole("option", { name: "Old page" }),
    ).toBeNull();
    expect(
      within(follows).queryByRole("option", { name: "Database" }),
    ).toBeNull();
    expect(
      within(follows).getByRole("option", { name: "No page, only PACT.md" }),
    ).toHaveValue("");
    await userEvent.selectOptions(follows, "");
    await userEvent.click(within(panel).getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(sent("PATCH", "/domains/eng/reflection/settings")).toEqual([
        { path: PAGE, follows: "" },
      ]);
    });
  });

  it("offers a custom freshness, says m is months, and shows the server's refusal", async () => {
    serve("curate", {
      "/domains/eng/reflection/settings": () => {
        throw new ApiProblem(
          422,
          "unprocessable",
          "freshness '0h' is shorter than 1 hour",
        );
      },
    });
    renderApp(URL);
    const panel = await edit(await details());
    await userEvent.selectOptions(
      within(panel).getByRole("combobox", { name: "Freshness" }),
      "custom",
    );
    const custom = within(panel).getByRole("textbox", {
      name: "Custom freshness",
    });
    expect(custom).toHaveAccessibleDescription(/m means months, not minutes/);
    await userEvent.type(custom, "0h");
    await userEvent.click(within(panel).getByRole("button", { name: "Save" }));
    expect(await within(panel).findByRole("alert")).toHaveTextContent(
      "freshness '0h' is shorter than 1 hour",
    );
    // A refusal keeps the form open, with what was typed.
    expect(custom).toHaveValue("0h");
  });

  it("has no freshness control on an owned page", async () => {
    serve("curate", {
      [DOC]: () =>
        doc(
          view({
            freshness: null,
            owner: {
              path: "arch/overview.md",
              title: "Overview",
              exists: true,
            },
          }),
        ),
    });
    renderApp(URL);
    const panel = await edit(await details());
    expect(
      within(panel).getByRole("combobox", { name: "Follows" }),
    ).toBeVisible();
    expect(
      within(panel).queryByRole("combobox", { name: "Freshness" }),
    ).toBeNull();
  });

  it("creates the page pact for a curator and opens an existing one", async () => {
    serve("curate", {
      "/domains/eng/reflections/arch/overview/db.pact.md": () => ({
        checksum: "p1",
        created: true,
      }),
    });
    const first = renderApp(URL);
    const panel = await details();
    await userEvent.click(
      within(panel).getByRole("button", { name: "Create the page pact" }),
    );
    await waitFor(() => {
      expect(
        sent("PUT", "/domains/eng/reflections/arch/overview/db.pact.md"),
      ).toEqual([{ markdown: "# Pact for Database\n", create: true }]);
    });
    first.unmount();
    serve("read", {
      [DOC]: () =>
        doc(view({ pact: { path: "arch/overview/db.pact.md", exists: true } })),
    });
    renderApp(URL);
    expect(
      within(await details()).getByRole("link", { name: "db.pact.md" }),
    ).toHaveAttribute("href", "/d/eng/reflection/arch/overview/db.pact.md");
  });

  it("shows the settings to a curator behind Edit, and Cancel and Escape go back to the view", async () => {
    serve("curate");
    renderApp(URL);
    const panel = await details();
    expect(within(panel).queryByRole("combobox")).toBeNull();
    const form = await edit(panel);
    await userEvent.selectOptions(
      within(form).getByRole("combobox", { name: "Freshness" }),
      "6h",
    );
    await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    expect(within(panel).queryByRole("form")).toBeNull();
    expect(within(panel).getByText("Freshness").nextSibling).toHaveTextContent(
      /^1 day$/,
    );
    expect(
      within(panel).getByRole("button", { name: "Edit page settings" }),
    ).toHaveFocus();
    // The form starts again from the stored settings.
    const again = await edit(panel);
    expect(
      within(again).getByRole("combobox", { name: "Freshness" }),
    ).toHaveValue("1d");
    await userEvent.keyboard("{Escape}");
    expect(within(panel).queryByRole("form")).toBeNull();
    expect(
      within(panel).getByRole("button", { name: "Edit page settings" }),
    ).toHaveFocus();
    expect(sent("PATCH", "/domains/eng/reflection/settings")).toEqual([]);
  });

  it("shows the saved settings after Save, with the focus back on Edit", async () => {
    let freshness = "1d";
    serve("curate", {
      [DOC]: () => doc(view({ freshness })),
      "/domains/eng/reflection/settings": () => {
        freshness = "6h";
        return {
          domain: "eng",
          path: PAGE,
          checksum: "c2",
          page: view({ freshness }),
        };
      },
    });
    renderApp(URL);
    const panel = await details();
    const form = await edit(panel);
    await userEvent.selectOptions(
      within(form).getByRole("combobox", { name: "Freshness" }),
      "6h",
    );
    await userEvent.click(within(form).getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(within(panel).queryByRole("form")).toBeNull();
    });
    expect(within(panel).getByText("Freshness").nextSibling).toHaveTextContent(
      /^6 hours$/,
    );
    expect(
      within(panel).getByRole("button", { name: "Edit page settings" }),
    ).toHaveFocus();
    // The save is said to a screen reader, until the next Edit.
    expect(within(panel).getByRole("status")).toHaveTextContent(/^Saved$/);
    await edit(panel);
    expect(within(panel).getByRole("status")).toHaveTextContent(/^$/);
  });

  it("says nothing was saved after a Cancel", async () => {
    serve("curate");
    renderApp(URL);
    const panel = await details();
    const form = await edit(panel);
    await userEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    expect(within(panel).getByRole("status")).toHaveTextContent(/^$/);
  });

  it("sends only what the curator changed, after another curator saved", async () => {
    let follows: string | null = null;
    serve("curate", {
      [DOC]: () => doc(view({ follows })),
      "/domains/eng/reflection/settings": () => ({
        domain: "eng",
        path: PAGE,
        checksum: "c2",
        page: view({ freshness: "6h", follows }),
      }),
    });
    renderApp(URL);
    const panel = await details();
    const form = await edit(panel);
    await userEvent.selectOptions(
      within(form).getByRole("combobox", { name: "Freshness" }),
      "6h",
    );
    // Another curator makes the page follow the overview, and the page is
    // read again while the form is open.
    follows = "arch/overview.md";
    const reads = (): number =>
      apiMock.mock.calls.filter(([p]) => p === DOC).length;
    const before = reads();
    focusManager.setFocused(false);
    focusManager.setFocused(true);
    await waitFor(() => {
      expect(reads()).toBeGreaterThan(before);
    });
    focusManager.setFocused(undefined);
    await userEvent.click(within(form).getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(sent("PATCH", "/domains/eng/reflection/settings")).toEqual([
        { path: PAGE, freshness: "6h" },
      ]);
    });
  });

  it("keeps the form open on an Escape that ends a composition", async () => {
    serve("curate");
    renderApp(URL);
    const panel = await details();
    const form = await edit(panel);
    fireEvent.keyDown(
      within(form).getByRole("combobox", { name: "Freshness" }),
      { key: "Escape", isComposing: true },
    );
    expect(within(panel).getByRole("form")).toBeInTheDocument();
    // Safari: the composition has ended, but the key code still says IME.
    fireEvent.keyDown(
      within(form).getByRole("combobox", { name: "Freshness" }),
      { key: "Escape", keyCode: 229 },
    );
    expect(within(panel).getByRole("form")).toBeInTheDocument();
  });

  it("closes the form on a retire, and leaves it closed after a restore", async () => {
    let retired = false;
    serve("curate", {
      [DOC]: () =>
        doc(
          retired
            ? view({ retired: { at: null, by: null, was_owned_by: null } })
            : view(),
        ),
      "/domains/eng/reflection/clock": () => ({ clock: null, checksum: "c1" }),
      "/domains/eng/reflection/retire": () => {
        retired = true;
        return { domain: "eng", path: PAGE, retired: true };
      },
      "/domains/eng/reflection/restore": () => {
        retired = false;
        return { domain: "eng", path: PAGE, restored: true };
      },
    });
    renderApp(URL);
    const panel = await details();
    await edit(panel);
    await moreActions();
    await userEvent.click(screen.getByRole("menuitem", { name: "Retire" }));
    const banner = await screen.findByRole("note", { name: "Retired" });
    await waitFor(() => {
      expect(within(panel).queryByRole("form")).toBeNull();
    });
    await userEvent.click(
      within(banner).getByRole("button", { name: "Restore" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("note", { name: "Retired" })).toBeNull();
    });
    await within(panel).findByRole("button", { name: "Edit page settings" });
    expect(within(panel).queryByRole("form")).toBeNull();
    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: "More actions" }),
      ).toHaveFocus();
    });
  });

  it("says no old save after a retire and a restore", async () => {
    let retired = false;
    let freshness = "1d";
    serve("curate", {
      [DOC]: () =>
        doc(
          retired
            ? view({
                freshness,
                retired: { at: null, by: null, was_owned_by: null },
              })
            : view({ freshness }),
        ),
      "/domains/eng/reflection/clock": () => ({ clock: null, checksum: "c1" }),
      "/domains/eng/reflection/settings": () => {
        freshness = "6h";
        return {
          domain: "eng",
          path: PAGE,
          checksum: "c2",
          page: view({ freshness }),
        };
      },
      "/domains/eng/reflection/retire": () => {
        retired = true;
        return { domain: "eng", path: PAGE, retired: true };
      },
      "/domains/eng/reflection/restore": () => {
        retired = false;
        return { domain: "eng", path: PAGE, restored: true };
      },
    });
    renderApp(URL);
    const panel = await details();
    const form = await edit(panel);
    await userEvent.selectOptions(
      within(form).getByRole("combobox", { name: "Freshness" }),
      "6h",
    );
    await userEvent.click(within(form).getByRole("button", { name: "Save" }));
    await waitFor(() => {
      expect(within(panel).getByRole("status")).toHaveTextContent(/^Saved$/);
    });
    await moreActions();
    await userEvent.click(screen.getByRole("menuitem", { name: "Retire" }));
    const banner = await screen.findByRole("note", { name: "Retired" });
    expect(within(panel).getByRole("status")).toHaveTextContent(/^$/);
    await userEvent.click(
      within(banner).getByRole("button", { name: "Restore" }),
    );
    await within(panel).findByRole("button", { name: "Edit page settings" });
    expect(within(panel).getByRole("status")).toHaveTextContent(/^$/);
  });

  it("offers no Edit on a retired page", async () => {
    serve("curate", {
      [DOC]: () =>
        doc(view({ retired: { at: null, by: null, was_owned_by: null } })),
      "/domains/eng/reflection/clock": () => ({ clock: null, checksum: "c1" }),
    });
    renderApp(URL);
    await screen.findByRole("note", { name: "Retired" });
    expect(
      within(await details()).queryByRole("button", {
        name: "Edit page settings",
      }),
    ).toBeNull();
  });

  it("keeps no lifecycle buttons in the column", async () => {
    serve("curate");
    renderApp(URL);
    const panel = await details();
    await within(panel).findByRole("button", { name: "Edit page settings" });
    for (const name of ["Retire", "Restore", "Delete"]) {
      expect(within(panel).queryByRole("button", { name })).toBeNull();
    }
  });

  it("retires a page for a curator from the More actions menu", async () => {
    serve("curate", {
      "/domains/eng/reflection/retire": () => ({
        domain: "eng",
        path: PAGE,
        retired: true,
      }),
    });
    renderApp(URL);
    await details();
    await moreActions();
    await userEvent.click(screen.getByRole("menuitem", { name: "Retire" }));
    await waitFor(() => {
      expect(sent("POST", "/domains/eng/reflection/retire")).toEqual([
        { path: PAGE },
      ]);
    });
  });

  it("says why a retire failed, and offers Retire again", async () => {
    serve("curate", {
      "/domains/eng/reflection/retire": () => {
        throw new ApiProblem(409, "conflict", "the page is open in a room");
      },
    });
    renderApp(URL);
    await details();
    await moreActions();
    await userEvent.click(screen.getByRole("menuitem", { name: "Retire" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "the page is open in a room",
    );
    await moreActions();
    expect(screen.getByRole("menuitem", { name: "Retire" })).toBeVisible();
  });

  it("offers no second Retire while one is on its way", async () => {
    serve("curate", {
      "/domains/eng/reflection/retire": () => new Promise(() => {}),
    });
    renderApp(URL);
    await details();
    await moreActions();
    await userEvent.click(screen.getByRole("menuitem", { name: "Retire" }));
    await waitFor(() => {
      expect(sent("POST", "/domains/eng/reflection/retire")).toHaveLength(1);
    });
    await moreActions();
    expect(screen.getByRole("menuitem", { name: "Move" })).toBeVisible();
    expect(screen.queryByRole("menuitem", { name: "Retire" })).toBeNull();
  });

  it("offers Retire in the menu only to a curator who may change the page", async () => {
    serve("edit");
    const first = renderApp(URL);
    await details();
    await moreActions();
    expect(screen.getByRole("menuitem", { name: "Move" })).toBeVisible();
    expect(screen.queryByRole("menuitem", { name: "Retire" })).toBeNull();
    await userEvent.keyboard("{Escape}");
    first.unmount();
    serve("curate", {}, { readOnly: true });
    const second = renderApp(URL);
    await details();
    await moreActions();
    expect(screen.queryByRole("menuitem", { name: "Retire" })).toBeNull();
    await userEvent.keyboard("{Escape}");
    second.unmount();
    serve("curate", {
      [DOC]: () =>
        doc(view({ retired: { at: null, by: null, was_owned_by: null } })),
      "/domains/eng/reflection/clock": () => ({ clock: null, checksum: "c1" }),
    });
    renderApp(URL);
    await screen.findByRole("note", { name: "Retired" });
    await moreActions();
    expect(screen.getByRole("menuitem", { name: "Move" })).toBeVisible();
    expect(screen.queryByRole("menuitem", { name: "Retire" })).toBeNull();
  });

  it("hands focus to the banner's Restore after a retire, and to More actions after a restore", async () => {
    let retired = false;
    serve("curate", {
      [DOC]: () =>
        doc(
          retired
            ? view({ retired: { at: null, by: null, was_owned_by: null } })
            : view(),
        ),
      "/domains/eng/reflection/clock": () => ({ clock: null, checksum: "c1" }),
      "/domains/eng/reflection/retire": () => {
        retired = true;
        return { domain: "eng", path: PAGE, retired: true };
      },
      "/domains/eng/reflection/restore": () => {
        retired = false;
        return { domain: "eng", path: PAGE, restored: true };
      },
    });
    renderApp(URL);
    await details();
    await moreActions();
    await userEvent.click(screen.getByRole("menuitem", { name: "Retire" }));
    const banner = await screen.findByRole("note", { name: "Retired" });
    await waitFor(() => {
      expect(
        within(banner).getByRole("button", { name: "Restore" }),
      ).toHaveFocus();
    });
    await userEvent.click(
      within(banner).getByRole("button", { name: "Restore" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("note", { name: "Retired" })).toBeNull();
    });
    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: "More actions" }),
      ).toHaveFocus();
    });
    await moreActions();
    expect(screen.getByRole("menuitem", { name: "Retire" })).toBeVisible();
  });

  it("deletes from the menu only on the second press, and keeps the page on Keep", async () => {
    serve("curate");
    renderApp(URL);
    await details();
    await moreActions();
    await userEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    await userEvent.click(screen.getByRole("button", { name: "Keep" }));
    expect(
      apiMock.mock.calls.some(([, init]) => init?.method === "DELETE"),
    ).toBe(false);
    expect(screen.getByRole("button", { name: "More actions" })).toHaveFocus();
    await moreActions();
    await userEvent.click(screen.getByRole("menuitem", { name: "Delete" }));
    await userEvent.click(
      screen.getByRole("button", { name: "Confirm delete" }),
    );
    await waitFor(() => {
      expect(
        apiMock.mock.calls.some(
          ([p, init]) => p === DOC && init?.method === "DELETE",
        ),
      ).toBe(true);
    });
  });

  it("offers nothing to change on a read-only instance", async () => {
    serve("curate", {}, { readOnly: true });
    renderApp(URL);
    const panel = await details();
    expect(within(panel).queryByRole("button")).toBeNull();
  });

  it("is not drawn for a pact file, which is no page", async () => {
    serve("curate", {
      "/domains/eng/reflections/arch/overview/db.pact.md": () => ({
        ...doc(null),
        path: "arch/overview/db.pact.md",
        title: "Pact for Database",
      }),
    });
    renderApp("/d/eng/reflection/arch/overview/db.pact.md");
    expect(
      await screen.findByRole("heading", { name: "Database", level: 1 }),
    ).toBeVisible();
    expect(
      screen.queryByRole("complementary", { name: "Page details" }),
    ).toBeNull();
    expect(screen.queryByTestId("page-status")).toBeNull();
    expect(
      apiMock.mock.calls.some(([p]) =>
        p.startsWith("/domains/eng/reflection/clock"),
      ),
    ).toBe(false);
  });
});

describe("a retired page", () => {
  const retired = (wasOwnedBy: Record<string, unknown> | null) =>
    view({
      status: "archived",
      retired: { at: null, by: null, was_owned_by: wasOwnedBy },
    });

  it("says it is no longer part of its owner and restores it there", async () => {
    serve("curate", {
      [DOC]: () =>
        doc(
          retired({
            path: "arch/overview.md",
            title: "Architecture overview",
            exists: true,
          }),
        ),
      "/domains/eng/reflection/clock": () => ({ clock: null, checksum: "c1" }),
      "/domains/eng/reflection/restore": () => ({
        domain: "eng",
        path: PAGE,
        restored: true,
      }),
    });
    renderApp(URL);
    const banner = await screen.findByRole("note", { name: "Retired" });
    expect(banner).toHaveTextContent(
      "Retired: no longer part of Architecture overview",
    );
    await userEvent.click(
      within(banner).getByRole("button", {
        name: "Restore under Architecture overview",
      }),
    );
    await waitFor(() => {
      expect(sent("POST", "/domains/eng/reflection/restore")).toEqual([
        { path: PAGE, owner: "arch/overview.md" },
      ]);
    });
    await userEvent.click(
      within(banner).getByRole("button", { name: "Restore" }),
    );
    await waitFor(() => {
      expect(sent("POST", "/domains/eng/reflection/restore")).toHaveLength(2);
    });
    expect(sent("POST", "/domains/eng/reflection/restore")[1]).toEqual({
      path: PAGE,
    });
  });

  it("says just retired for a page on its own, and offers a reader no control", async () => {
    serve("edit", {
      [DOC]: () => doc(retired(null)),
      "/domains/eng/reflection/clock": () => ({ clock: null, checksum: "c1" }),
    });
    renderApp(URL);
    const banner = await screen.findByRole("note", { name: "Retired" });
    expect(banner).toHaveTextContent(/^Retired$/);
    expect(within(banner).queryByRole("button")).toBeNull();
  });

  it("offers no restore under an owner that is gone", async () => {
    serve("curate", {
      [DOC]: () =>
        doc(
          retired({
            path: "arch/gone.md",
            title: "arch/gone.md",
            exists: false,
          }),
        ),
      "/domains/eng/reflection/clock": () => ({ clock: null, checksum: "c1" }),
    });
    renderApp(URL);
    const banner = await screen.findByRole("note", { name: "Retired" });
    expect(
      within(banner).getByRole("button", { name: "Restore" }),
    ).toBeVisible();
    expect(
      within(banner).queryByRole("button", { name: /Restore under/ }),
    ).toBeNull();
  });
});

describe("the text is read again only when it moved", () => {
  const reads = () => apiMock.mock.calls.filter(([p]) => p === DOC).length;
  /** The clock answers a little after the page, as a refetch on a change does. */
  const later = (checksum: string) => async () => {
    await new Promise((resolve) => setTimeout(resolve, 30));
    return { clock: clockOf(), checksum };
  };

  it("reads the page once when the clock names the checksum it holds", async () => {
    serve("read", { "/domains/eng/reflection/clock": later("c1") });
    renderApp(URL);
    expect(
      await screen.findByText("Last checked 2 hours ago, due again in 1 hour"),
    ).toBeVisible();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(reads()).toBe(1);
  });

  it("reads the page again when the clock names another checksum", async () => {
    serve("read", {
      "/domains/eng/reflection/clock": later("c2"),
      // The first read is the text before the change; the next one after.
      [DOC]: () => doc(view(), reads() <= 1 ? "c1" : "c2"),
    });
    renderApp(URL);
    await waitFor(() => {
      expect(reads()).toBe(2);
    });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(reads()).toBe(2);
  });
});
