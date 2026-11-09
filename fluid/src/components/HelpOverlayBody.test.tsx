/**
 * The shortcut map names each key as this keyboard has it. jsdom's platform
 * is "", which is no Mac, so it says Ctrl; `HelpOverlayBody.mac.test.tsx` is
 * the Mac's side.
 */

import { render, screen, within } from "@testing-library/react";
import { expect, it } from "vitest";

import HelpOverlayBody from "./HelpOverlayBody";

it("names Ctrl off a Mac, and no Cmd", () => {
  render(<HelpOverlayBody onClose={() => undefined} returnFocusTo={null} />);
  const map = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
  for (const keys of [
    "Ctrl+K",
    "Ctrl+S",
    "Ctrl+F",
    "Ctrl+B",
    "Ctrl+I",
    "Esc",
  ]) {
    expect(within(map).getByText(keys, { selector: "kbd" })).toBeVisible();
  }
  expect(within(map).queryByText(/Cmd|⌘/)).toBeNull();
});

it("names the menu key and the toolbar key, with what they do", () => {
  render(<HelpOverlayBody onClose={() => undefined} returnFocusTo={null} />);
  const map = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
  const does = (keys: string) =>
    within(map)
      .getByText(keys, { selector: "kbd" })
      .closest("div")
      ?.querySelector("dd")?.textContent;
  expect(does("Shift+F10")).toBe(
    "Menu for the item you are on (in the editor)",
  );
  expect(does("Alt+F10")).toBe(
    "Toolbar for the selection (in the editor and on a Reflection page)",
  );
});
