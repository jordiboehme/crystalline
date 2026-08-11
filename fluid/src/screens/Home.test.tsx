/**
 * The screen the app opens on: what this instance knows about, and what has
 * been learned lately.
 *
 * The cards are the answer to "what is in here", so what they say has to come
 * from the listing rather than from a guess: the count, the routing line the
 * MANIFEST carries, and a date that names which fact it is - the last thing
 * recorded, or, when nothing was, the last sync.
 */

import { screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
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

/** The activity payload, in the engine's own shape. */
function activityResponse() {
  return {
    timeframe: "7d",
    count: 1,
    engrams: [
      {
        domain: "eng",
        permalink: "notes/beta",
        title: "Beta",
        engram_type: "decision",
        status: "stable",
        tags: ["eng"],
        recorded_at: "2026-08-04",
      },
    ],
  };
}

function serve(routes: Record<string, () => unknown> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
      "/activity": activityResponse,
      ...routes,
    }),
  );
}

/** The screen's own region, so the sidebar's domain links are out of scope. */
async function main() {
  return within(await screen.findByRole("main"));
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the home screen", () => {
  it("gives every domain a card with its count and its routing line", async () => {
    serve();

    renderApp("/");

    const screenMain = await main();
    const card = await screenMain.findByRole("link", { name: "eng" });
    expect(card).toHaveAttribute("href", "/d/eng");
    expect(await screenMain.findByText("4 engrams")).toBeVisible();
    expect(
      await screenMain.findByText("Route here for eng questions."),
    ).toBeVisible();
    // What backs the domain is a fact about it, so it wears the same chip
    // every other fact in this app wears.
    expect(screenMain.getByText("file").className).toContain("slate");
  });

  it("leaves the tagline to the screen that owns it", async () => {
    serve();

    renderApp("/");

    // Said once, on the way in. A subtitle repeating it under every visit is
    // the kind of line a reader stops seeing.
    await (await main()).findByRole("heading", { name: "Home", level: 1 });
    expect(screen.queryByText(/where you think with it/)).toBeNull();
  });

  it("dates a card by what was recorded, not by what was guessed", async () => {
    serve();

    renderApp("/");

    // The feed puts an engram in `eng` on 2026-08-04, so that is the card's
    // last activity. A domain the feed never mentions gets no invented date.
    expect(
      await (await main()).findByText("Last activity 2026-08-04"),
    ).toBeVisible();
  });

  it("lists what was recorded lately, linked to the engrams", async () => {
    serve();

    renderApp("/");

    const feed = await screen.findByRole("region", {
      name: /Recent activity/,
    });
    const entry = await within(feed).findByRole("link", { name: /Beta/ });
    expect(entry).toHaveAttribute("href", "/d/eng/e/notes/beta");
    expect(within(feed).getByText(/7d/)).toBeVisible();
  });

  it("says the feed is empty rather than showing an empty box", async () => {
    serve({
      "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
    });

    renderApp("/");

    expect(
      await screen.findByText(/Nothing was recorded in the last 7d/),
    ).toBeVisible();
    // An empty feed is a state with a way out of it: what fills it, and a
    // door into the first domain there is.
    expect(
      screen.getByText(
        "Activity appears as engrams are written, edited or verified.",
      ),
    ).toBeVisible();
    const feed = await screen.findByRole("region", { name: /Recent activity/ });
    expect(
      within(feed).getByRole("link", { name: "Start in eng" }),
    ).toHaveAttribute("href", "/d/eng");
  });

  it("offers no door into a domain when there are none", async () => {
    serve({
      "/domains": () => ({ behavior: [], domains: [] }),
      "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
    });

    renderApp("/");

    await screen.findByText(/Nothing was recorded in the last 7d/);
    expect(screen.queryByRole("link", { name: /^Start in/ })).toBeNull();
  });
});
