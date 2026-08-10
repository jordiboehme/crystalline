import { render, screen } from "@testing-library/react";
import { Gem } from "lucide-react";
import { describe, expect, test } from "vitest";

import { BUTTON, Chip, IconButton, statusVariant } from "./primitives";

describe("primitives", () => {
  test("an icon button is named for the screen reader and the pointer", () => {
    render(<IconButton label="Copy address" icon={Gem} />);
    const button = screen.getByRole("button", { name: "Copy address" });
    expect(button).toHaveAttribute("title", "Copy address");
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
});
