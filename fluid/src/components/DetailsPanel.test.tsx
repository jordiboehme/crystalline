/**
 * What the quiet column beside an engram is allowed to claim.
 *
 * Every row is a field the engram carries, and a field it does not carry has
 * no row: an absent `valid_from` means the knowledge has always been valid and
 * an absent `valid_to` means it is valid forever, so the honest rendering of
 * both is nothing at all rather than a sentence or a date nobody wrote.
 */

import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { describe, expect, test } from "vitest";

import type { EngramFrontmatter } from "../api/engram";
import { DetailsPanel } from "./DetailsPanel";
import { Tooltips } from "./primitives";

const FRONTMATTER: EngramFrontmatter = {
  type: "guide",
  status: "current",
  tags: ["protocol", "smoke"],
  salience: 0.7,
  validFrom: null,
  validTo: null,
  staleAfter: null,
  verified: [],
  generatedBy: null,
  generatedModel: null,
};

function draw(overrides: Partial<EngramFrontmatter> = {}) {
  render(
    <MemoryRouter>
      <DetailsPanel
        frontmatter={{ ...FRONTMATTER, ...overrides }}
        address="crystalline://playground/lantern-protocol"
      />
    </MemoryRouter>,
    // The copy button is an `IconButton`, which names itself with a tooltip,
    // and a tooltip belongs to a group. The app mounts that group once at its
    // root; a suite that mounts one panel on its own mounts it here.
    { wrapper: Tooltips },
  );
}

describe("DetailsPanel", () => {
  test("status wears a filled chip, type a neutral one, tags link to search", () => {
    draw();
    expect(screen.getByText("current").className).toContain("emerald");
    expect(screen.getByText("guide")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "#protocol" })).toHaveAttribute(
      "href",
      "/search?tags=protocol",
    );
  });

  test("absent validity renders no row at all - never a sentinel", () => {
    draw();
    expect(screen.queryByText(/Valid/)).toBeNull();
    expect(screen.queryByText(/forever/i)).toBeNull();
    expect(screen.queryByText(/9999/)).toBeNull();
  });

  test("a bounded engram states each end the engram states", () => {
    draw({ validFrom: "2026-01-02", validTo: null });
    expect(screen.getByText("from 2026-01-02")).toBeInTheDocument();
  });

  test("the latest verification is the stamp that speaks for the engram", () => {
    draw({
      verified: [
        { by: "human:ada", model: null, at: "2026-01-01T09:00:00+01:00" },
        { by: "human:jordi", model: null, at: "2026-02-01T10:00:00+01:00" },
      ],
    });
    expect(screen.getByText("jordi (human) on 2026-02-01")).toBeInTheDocument();
  });

  test("who captured the engram is stated in a reader's words", () => {
    draw({ generatedBy: "human:jordi" });
    expect(screen.getByText("Captured by")).toBeInTheDocument();
    expect(screen.getByText("jordi (human)")).toBeInTheDocument();
  });

  test("shows the model beside the writer", () => {
    draw({
      generatedBy: "claude-code/2.1.271",
      generatedModel: "claude-opus-5",
    });
    expect(
      screen.getByText("claude-code (agent, 2.1.271) with claude-opus-5"),
    ).toBeInTheDocument();
  });

  test("shows a verification's model", () => {
    draw({
      verified: [
        {
          by: "claude-code/2.1.271",
          model: "claude-opus-5",
          at: "2026-02-01T10:00:00+01:00",
        },
      ],
    });
    expect(
      screen.getByText(
        "claude-code (agent, 2.1.271) with claude-opus-5 on 2026-02-01",
      ),
    ).toBeInTheDocument();
  });

  test("the captured-by and verified-by writers are formatted the same way", () => {
    // Both rows name the same actor, so a reader who is not the same person
    // reading both rows should not see "Jordi" in one and "human:jordi" in
    // the other - the raw stored form must never surface here.
    draw({
      generatedBy: "human:jordi",
      verified: [
        { by: "human:jordi", model: null, at: "2026-02-01T10:00:00+01:00" },
      ],
    });
    expect(screen.queryByText(/human:jordi/)).toBeNull();
    expect(screen.getByText("jordi (human)")).toBeInTheDocument();
    expect(screen.getByText("jordi (human) on 2026-02-01")).toBeInTheDocument();
  });

  test("an engram that records no writer is attributed to nobody", () => {
    draw();
    expect(screen.queryByText("Captured by")).toBeNull();
  });

  test("a field the engram leaves out has no row", () => {
    draw({ type: null, tags: [], salience: null });
    expect(screen.queryByText("Type")).toBeNull();
    expect(screen.queryByText("Tags")).toBeNull();
    expect(screen.queryByText("Salience")).toBeNull();
    // The panel is still the panel: its own name and the address survive.
    expect(screen.getByRole("region", { name: "Details" })).toBeInTheDocument();
  });

  test("a salience of zero is a field the engram carries", () => {
    // Zero is a written salience, not a missing one: the row is drawn only
    // where the engram states the field, and the check that decides it has to
    // be about absence rather than about truthiness.
    draw({ salience: 0 });
    expect(screen.getByText("Salience")).toBeInTheDocument();
    expect(screen.getByText("0")).toBeInTheDocument();
  });

  test("the address is shown and copiable", () => {
    draw();
    expect(
      screen.getByText("crystalline://playground/lantern-protocol"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Copy crystalline:// address" }),
    ).toBeInTheDocument();
    // The outcome is announced beside the control rather than written into
    // its label, so the control keeps the name a reader navigates by.
    expect(
      screen.getByRole("status", { name: "Copy address result" }),
    ).toBeInTheDocument();
  });
});
