import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Gem } from "lucide-react";
import { describe, expect, test } from "vitest";

import {
  BUTTON,
  Chip,
  IconButton,
  TOGGLE,
  Tooltips,
  statusVariant,
} from "./primitives";

/**
 * Long enough to outlast the tooltip's own hover delay, which is the point of
 * the delay: a pointer crossing a row of icons says nothing, a pointer that
 * stops gets an answer.
 */
const TOOLTIP_TIMEOUT_MS = 2000;

describe("primitives", () => {
  test("an icon button is named for the screen reader and the pointer", async () => {
    render(<IconButton label="Copy address" icon={Gem} />, {
      wrapper: Tooltips,
    });
    const button = screen.getByRole("button", { name: "Copy address" });
    // The accessible name is the label, and it is the label alone: the
    // tooltip describes, it does not rename.
    expect(button).toHaveAttribute("aria-label", "Copy address");
    // And no `title`, so the browser cannot draw a second one underneath.
    expect(button).not.toHaveAttribute("title");
    expect(screen.queryByRole("tooltip")).toBeNull();

    await userEvent.hover(button);
    // The delay is deliberate, so this waits it out rather than pretending a
    // tooltip is instant.
    const onHover = await screen.findByRole(
      "tooltip",
      {},
      { timeout: TOOLTIP_TIMEOUT_MS },
    );
    expect(onHover).toHaveTextContent("Copy address");
    // Still one name, not two: the tooltip is tied on as a description.
    expect(screen.getByRole("button", { name: "Copy address" })).toBe(button);
  });

  test("the keyboard gets the tooltip too, and without waiting", async () => {
    render(<IconButton label="Print view" icon={Gem} />, { wrapper: Tooltips });
    // Tab rather than `focus()`: this pins what a keyboard actually does, and
    // Radix opens on focus immediately - a control somebody moved to on
    // purpose has already asked its question.
    await userEvent.tab();
    expect(screen.getByRole("button", { name: "Print view" })).toHaveFocus();
    expect(await screen.findByRole("tooltip")).toHaveTextContent("Print view");
  });

  test("status maps to filled semantic variants, free-form falls back", () => {
    expect(statusVariant("current")).toBe("positive");
    expect(statusVariant("Stable")).toBe("positive");
    expect(statusVariant("draft")).toBe("caution");
    expect(statusVariant("deprecated")).toBe("retired");
    expect(statusVariant("anything-else")).toBe("neutral");
  });

  test("a chip renders its variant", () => {
    render(<Chip variant="positive">current</Chip>);
    expect(screen.getByText("current").className).toContain("emerald");
  });

  test("the primary tier is filled accent", () => {
    expect(BUTTON.primary).toContain("bg-accent-700");
  });

  test("a pressed toggle declares its own colors instead of layering", () => {
    // The regression this pins: `${BUTTON.ghost} bg-accent-50 text-accent-800`
    // renders as plain ghost, because Tailwind emits `.text-slate-600` after
    // `.text-accent-800` and ghost's `hover:bg-slate-100` (0,2,0) outranks a
    // bare `bg-accent-50` (0,1,0). A pressed face may therefore carry NO
    // utility from the family the off face already spends.
    expect(TOGGLE.on).toContain("bg-accent-100");
    expect(TOGGLE.on).toContain("text-accent-900");
    expect(TOGGLE.on).not.toMatch(/(^|\s|:)text-slate-/);
    expect(TOGGLE.on).not.toMatch(/(^|\s|:)bg-slate-/);
    // And it owns its own hover, so the pointer cannot erase the state.
    expect(TOGGLE.on).toContain("hover:bg-accent-200");
    expect(TOGGLE.off).not.toContain("accent-100");
    // Both faces reserve the border, so pressing recolors and moves nothing.
    // The wash alone is 1.13:1 against a white page - below the non-text
    // floor - so the border is the state indicator that actually carries.
    expect(TOGGLE.on).toContain("border border-accent-600");
    expect(TOGGLE.off).toContain("border border-transparent");
  });
});
