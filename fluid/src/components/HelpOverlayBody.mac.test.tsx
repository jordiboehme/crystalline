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

it("names the menu key and the toolbar key, with what they do", () => {
  render(<HelpOverlayBody onClose={() => undefined} returnFocusTo={null} />);
  const map = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
  const does = (keys: string) =>
    within(map)
      .getByText(keys, { selector: "kbd" })
      .closest("div")
      ?.querySelector("dd")?.textContent;
  expect(does("⌃↩")).toBe("Menu for the item you are on (in the editor)");
  expect(does("⌥F10")).toBe(
    "Toolbar for the selection (in the editor and on a Reflection page)",
  );
});
