/**
 * The shortcut map on a Mac names Cmd as the Mac shows it, ⌘. The platform is
 * read once, when the modules load, so it is set before any import.
 */

import { expect, it, vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "MacIntel",
    configurable: true,
  });
});

import { render, screen, within } from "@testing-library/react";

import HelpOverlayBody from "./HelpOverlayBody";

it("names ⌘ on a Mac, and no Ctrl", () => {
  render(<HelpOverlayBody onClose={() => undefined} returnFocusTo={null} />);
  const map = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
  for (const keys of ["⌘K", "⌘S", "⌘F", "⌘B", "⌘I", "Esc"]) {
    expect(within(map).getByText(keys, { selector: "kbd" })).toBeVisible();
  }
  expect(within(map).queryByText(/Ctrl/)).toBeNull();
});
