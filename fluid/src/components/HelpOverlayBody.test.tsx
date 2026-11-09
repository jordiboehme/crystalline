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
