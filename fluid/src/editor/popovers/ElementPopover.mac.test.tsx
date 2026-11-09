/**
 * An element popover on a Mac takes its own menu key: Ctrl+Return opened it, and
 * a second or held press must run nothing inside it (R111, M6). The platform is
 * read once, when the modules load, so it is set before any import.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "MacIntel",
    configurable: true,
  });
});

import { cleanup, fireEvent, screen } from "@testing-library/react";

import { bytes, cleanupMarkViews, keys, setCaret } from "../../test/markKit";
import { popoverView } from "../../test/popoverKit";

afterEach(() => {
  cleanup();
  cleanupMarkViews();
});

/** Ctrl+Return on `el`, as a browser sends it; whether the page took it. */
function ctrlReturn(el: Element, repeat = false): boolean {
  return !fireEvent.keyDown(el, { key: "Enter", ctrlKey: true, repeat });
}

describe("a second Ctrl+Return in an open popover on a Mac", () => {
  it("writes no colour from the status label's radios", async () => {
    const view = popoverView("x\n:status[Done]{green}");
    setCaret(view, 10);
    expect(keys(view, "Ctrl-Enter")).toBe(true);
    await screen.findByRole("dialog", { name: "Status label" });
    const red = screen.getByRole("radio", { name: "red" });
    fireEvent.click(red);
    expect(ctrlReturn(red)).toBe(true);
    ctrlReturn(red, true);
    expect(bytes(view.state)).toBe("x\n:status[Done]{green}");
    expect(screen.getByRole("dialog", { name: "Status label" })).toBeVisible();
  });

  it("sets no task state from the task menu", async () => {
    const view = popoverView("x\n- [ ] t");
    setCaret(view, 8);
    keys(view, "Ctrl-Enter");
    const menu = await screen.findByRole("menu", { name: "Task state" });
    const item = document.activeElement as HTMLElement;
    expect(menu.contains(item)).toBe(true);
    expect(ctrlReturn(item)).toBe(true);
    expect(bytes(view.state)).toBe("x\n- [ ] t");
    expect(screen.getByRole("menu", { name: "Task state" })).toBeVisible();
  });
});
