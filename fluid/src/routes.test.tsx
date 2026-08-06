/**
 * The one route that can go wrong quietly: an engram's.
 *
 * A permalink is a path of its own, so the link builder, the URL and the splat
 * param have to agree about that all the way through. If any of the three
 * treats it as a single segment instead, `notes/deep/gamma` becomes
 * `notes%2Fdeep%2Fgamma` and the app navigates to a different, missing engram -
 * a 404 nobody can explain from the link they clicked. So this walks the whole
 * round trip through the real router rather than checking the encoder alone.
 */

import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Link } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "./api/client";
import { engramRoute } from "./paths";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "./test/harness";

vi.mock("./api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

beforeEach(() => {
  apiMock.mockReset();
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
    }),
  );
});

/** Click a link built for this permalink, from the home screen. */
async function followLinkTo(permalink: string) {
  renderApp(
    "/",
    <Link to={engramRoute("eng", permalink)}>Open the engram</Link>,
  );
  await screen.findByRole("heading", { name: "Home" });
  const link = screen.getByRole("link", { name: "Open the engram" });
  await userEvent.click(link);
  return link;
}

describe("the engram route", () => {
  it("carries a multi-segment permalink through the link and the splat", async () => {
    const link = await followLinkTo("notes/deep/gamma");

    // The slashes inside the permalink stay slashes in the URL, which is what
    // makes the splat match rather than a single escaped segment.
    expect(link).toHaveAttribute("href", "/d/eng/e/notes/deep/gamma");
    expect(
      await screen.findByRole("heading", {
        name: "Engram: notes/deep/gamma in eng",
      }),
    ).toBeVisible();
  });

  it("round-trips a segment that needed encoding, decoded", async () => {
    const link = await followLinkTo("notes/deep dive/gamma");

    expect(link).toHaveAttribute("href", "/d/eng/e/notes/deep%20dive/gamma");
    // Encoded on the way out, decoded on the way in: the screen sees the
    // permalink as it is written on disk, not as it travelled.
    expect(
      await screen.findByRole("heading", {
        name: "Engram: notes/deep dive/gamma in eng",
      }),
    ).toBeVisible();
  });

  it("carries a domain that needed encoding too", async () => {
    renderApp(
      "/",
      <Link to={engramRoute("team eng", "notes/alpha")}>Open the engram</Link>,
    );
    await screen.findByRole("heading", { name: "Home" });
    await userEvent.click(
      screen.getByRole("link", { name: "Open the engram" }),
    );

    expect(
      await screen.findByRole("heading", {
        name: "Engram: notes/alpha in team eng",
      }),
    ).toBeVisible();
  });
});
