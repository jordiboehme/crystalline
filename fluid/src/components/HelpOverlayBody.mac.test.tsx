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

it("names each key by its sign and in words on a Mac (M4)", () => {
  render(<HelpOverlayBody onClose={() => undefined} returnFocusTo={null} />);
  const map = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
  for (const keys of [
    "⌘K (Cmd+K)",
    "⌘S (Cmd+S)",
    "⌘F (Cmd+F)",
    "⌘B (Cmd+B)",
    "⌘I (Cmd+I)",
    "Esc",
  ]) {
    expect(within(map).getByText(keys, { selector: "kbd" })).toBeVisible();
  }
  expect(within(map).queryByText(/Ctrl\+K|Alt/)).toBeNull();
});

it("names the menu key and the toolbar key, with what they do", () => {
  render(<HelpOverlayBody onClose={() => undefined} returnFocusTo={null} />);
  const map = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
  const does = (keys: string) =>
    within(map)
      .getByText(keys, { selector: "kbd" })
      .closest("div")
      ?.querySelector("dd")?.textContent;
  expect(does("⌃↩ (Ctrl+Return)")).toBe(
    "Menu for the item you are on (in the editor)",
  );
  expect(does("⌥F10 (Option+F10)")).toBe(
    "Toolbar for the selection (in the editor and on a Reflection page)",
  );
});
