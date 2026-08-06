/**
 * What the engram page says about an engram's lifecycle, and what it refuses to
 * say.
 *
 * Two facts earn a banner and nothing else does. A retired status means the
 * engram is kept for the record rather than as current knowledge, and the way
 * out of it is the supersedes chain, so the banner carries it. A `stale_after`
 * that has passed means the knowledge is due for a check, which is a different
 * claim and gets its own line.
 *
 * The silence is the part worth pinning. Absent temporal fields mean "always
 * valid" and "valid forever" in this repo, so a live engram carrying no dates
 * renders nothing at all: a placeholder date here would be an invented fact,
 * and one an agent reading the screen would carry away as true.
 *
 * The chain has the same three states every reference in this app has, and the
 * middle one matters most here: a successor the index resolved but the graph
 * has not placed yet is named plainly. Calling it unresolved would be a false
 * claim, and on the ordinary load path it would be the first thing a reader of
 * a retired engram sees.
 */

import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { describe, expect, it } from "vitest";

import { LifecycleBanner } from "./LifecycleBanner";
import type { LifecycleBannerProps } from "./LifecycleBanner";

/** A live engram with nothing to announce, which each test moves one field of. */
function banner(overrides: Partial<LifecycleBannerProps> = {}) {
  const props: LifecycleBannerProps = {
    status: "stable",
    staleAfter: null,
    supersededBy: [],
    supersedes: [],
    today: "2026-08-06",
    ...overrides,
  };
  return render(
    <MemoryRouter>
      <LifecycleBanner {...props} />
    </MemoryRouter>,
  );
}

describe("the lifecycle banner", () => {
  it("names a retired status and links to what replaced it", () => {
    banner({
      status: "superseded",
      supersededBy: [
        { label: "Beta", href: "/d/eng/e/beta", state: "resolved" },
      ],
    });

    expect(screen.getByText(/superseded/)).toBeVisible();
    const successor = screen.getByRole("link", { name: "Beta" });
    expect(successor).toHaveAttribute("href", "/d/eng/e/beta");
  });

  it("links back along the supersedes chain", () => {
    banner({
      status: "archived",
      supersedes: [
        { label: "Alpha", href: "/d/eng/e/alpha", state: "resolved" },
      ],
    });

    expect(screen.getByText(/Supersedes/)).toBeVisible();
    expect(screen.getByRole("link", { name: "Alpha" })).toHaveAttribute(
      "href",
      "/d/eng/e/alpha",
    );
  });

  it("names a successor the index could not resolve without linking it", () => {
    banner({
      status: "superseded",
      supersededBy: [{ label: "Ghost", href: null, state: "unresolved" }],
    });

    // Named, because the engram says so; not a link, because nothing on this
    // instance answers to that name and a link that goes nowhere is a lie.
    expect(screen.getByText("Ghost")).toBeVisible();
    expect(screen.queryByRole("link", { name: "Ghost" })).toBeNull();
    expect(screen.getByTitle("not resolved")).toHaveTextContent("Ghost");
  });

  it("names a successor plainly while the graph has yet to place it", () => {
    banner({
      status: "superseded",
      supersededBy: [{ label: "Beta", href: null, state: "pending" }],
    });

    // The index resolved it, so calling it unresolved would be false. It has
    // no address yet, so linking it would be a guess. It is named, plainly and
    // with nothing hovering over it to claim otherwise, and it becomes a link
    // a moment later.
    const successor = screen.getByText("Beta");
    expect(successor).toBeVisible();
    expect(successor).not.toHaveAttribute("title");
    expect(screen.queryByRole("link", { name: "Beta" })).toBeNull();
  });

  it("says the knowledge is due for a check once its staleness date has passed", () => {
    banner({ staleAfter: "2026-01-01", today: "2026-08-06" });

    expect(screen.getByText(/2026-01-01/)).toBeVisible();
    expect(screen.getByRole("status")).toBeInTheDocument();
  });

  it("says nothing about a staleness date still ahead", () => {
    const { container } = banner({
      staleAfter: "2026-12-31",
      today: "2026-08-06",
    });

    expect(container).toBeEmptyDOMElement();
  });

  it("says nothing at all about a live engram carrying no dates", () => {
    const { container } = banner();

    expect(container).toBeEmptyDOMElement();
  });

  it("says both things when a retired engram is also overdue", () => {
    banner({
      status: "deprecated",
      staleAfter: "2026-02-02",
      supersededBy: [
        { label: "Beta", href: "/d/eng/e/beta", state: "resolved" },
      ],
    });

    expect(screen.getByText(/deprecated/)).toBeVisible();
    expect(screen.getByText(/2026-02-02/)).toBeVisible();
  });
});
