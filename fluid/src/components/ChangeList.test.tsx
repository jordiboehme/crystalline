/**
 * The rows of the change list, mounted on their own: what a press on a path
 * does, what the row menu offers, and the two things the list never draws -
 * a generated folder listing, and a refusal that belongs to another row.
 *
 * On their own rather than through the share dialog, because the dialog's own
 * tests pin the wiring and these pin the row: a menu that offers Discard to
 * somebody who may not discard is a defect the dialog could hide by never
 * passing the callback.
 */

import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { ChangeList } from "./ChangeList";
import { Tooltips } from "./primitives";

const CHANGES = [
  { path: "notes/a.md", kind: "modified", lastAuthor: null, sha: "9f2c" },
  { path: "notes/new.md", kind: "added", lastAuthor: null, sha: "51ab" },
  { path: "notes/index.md", kind: "modified", lastAuthor: null, sha: "c0de" },
];

function mount(overrides: Partial<Parameters<typeof ChangeList>[0]> = {}) {
  const onOpen = vi.fn();
  const onDiscard = vi.fn();
  render(
    <ChangeList
      changes={CHANGES}
      selected={new Set(["notes/a.md"])}
      onToggle={() => undefined}
      onToggleGroup={() => undefined}
      onOpen={onOpen}
      onDiscard={onDiscard}
      refusals={new Map()}
      {...overrides}
    />,
    { wrapper: Tooltips },
  );
  return { onOpen, onDiscard };
}

describe("the change list's rows", () => {
  it("makes the path a button that opens the pane, keeps the box and the badge", async () => {
    const { onOpen } = mount();
    await userEvent.click(screen.getByRole("button", { name: "notes/a.md" }));
    expect(onOpen).toHaveBeenCalledWith("notes/a.md");
    expect(screen.getByRole("checkbox", { name: "notes/a.md" })).toBeChecked();
    expect(screen.getByRole("img", { name: "Modified" })).toBeInTheDocument();
  });

  it("offers What changed and Discard behind a named menu", async () => {
    const { onOpen, onDiscard } = mount();
    await userEvent.click(
      screen.getByRole("button", { name: "Actions for notes/new.md" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "What changed" }),
    );
    expect(onOpen).toHaveBeenCalledWith("notes/new.md");
    await userEvent.click(
      screen.getByRole("button", { name: "Actions for notes/new.md" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Discard" }),
    );
    // The second argument is the trigger itself, which is what the strip
    // gives focus back to; the row hands it over rather than leaving the
    // caller to guess at `document.activeElement`, which is the menu item
    // while `onSelect` runs.
    expect(onDiscard).toHaveBeenCalledWith(
      "notes/new.md",
      screen.getByRole("button", { name: "Actions for notes/new.md" }),
    );
  });

  it("has no Discard item when the caller may not discard", async () => {
    mount({ onDiscard: undefined });
    await userEvent.click(
      screen.getByRole("button", { name: "Actions for notes/a.md" }),
    );
    expect(
      await screen.findByRole("menuitem", { name: "What changed" }),
    ).toBeVisible();
    expect(screen.queryByRole("menuitem", { name: "Discard" })).toBeNull();
  });

  it("draws a refusal under its row and never draws a listing", () => {
    mount({ refusals: new Map([["notes/a.md", "Changed since you looked."]]) });
    const box = screen.getByRole("checkbox", { name: "notes/a.md" });
    const row = box.closest("li");
    const reason = within(row as HTMLElement).getByText(
      "Changed since you looked.",
    );
    expect(reason).toBeInTheDocument();
    // And the row says it out loud: a reason nobody can see is a reason
    // nobody hears without this.
    expect(box).toHaveAttribute("aria-describedby", reason.id);
    expect(
      screen.getByRole("checkbox", { name: "notes/new.md" }),
    ).not.toHaveAttribute("aria-describedby");
    expect(screen.queryByText("notes/index.md")).toBeNull();
    expect(screen.queryByText(/folder index/)).toBeNull();
  });
});
