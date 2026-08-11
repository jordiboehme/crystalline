import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { describe, expect, test } from "vitest";

import { Breadcrumbs, crumbsOf } from "./Breadcrumbs";

describe("crumbsOf", () => {
  test("domain links home, folders are plain, the title is last", () => {
    const crumbs = crumbsOf(
      "playground",
      "notes/deep/gamma",
      "Deep Gamma Note",
    );
    expect(crumbs).toEqual([
      { label: "playground", href: "/d/playground" },
      { label: "notes", href: null },
      { label: "deep", href: null },
      { label: "Deep Gamma Note", href: null },
    ]);
  });

  test("a single-segment permalink has no folder crumbs", () => {
    expect(
      crumbsOf("playground", "lantern-protocol", "Lantern Protocol"),
    ).toEqual([
      { label: "playground", href: "/d/playground" },
      { label: "Lantern Protocol", href: null },
    ]);
  });
});

describe("Breadcrumbs", () => {
  test("renders a labelled trail with the leaf as the current page", () => {
    render(
      <MemoryRouter>
        <Breadcrumbs
          crumbs={crumbsOf("playground", "notes/deep/gamma", "Deep Gamma Note")}
        />
      </MemoryRouter>,
    );
    const nav = screen.getByRole("navigation", { name: "Breadcrumb" });
    expect(nav).toHaveTextContent("playground");
    expect(nav).toHaveTextContent("deep");
    expect(screen.getByText("Deep Gamma Note")).toHaveAttribute(
      "aria-current",
      "page",
    );
  });
});
