/**
 * The inline confirmation a discard asks for, on its own: what it says, what
 * it is focused on when it opens, and the three ways out of it.
 *
 * The screens that arm it pin where it appears; these pin the strip itself,
 * because every one of those screens relies on the same three things - the
 * sentence names what goes, the confirm is where the keyboard already is, and
 * Escape is a cancel rather than a press.
 */

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { DiscardConfirm } from "./DiscardConfirm";

describe("the discard strip", () => {
  it("says what goes, and opens on the confirm", () => {
    render(
      <DiscardConfirm
        question="Discard 3 files?"
        pending={false}
        problem={null}
        onConfirm={() => undefined}
        onCancel={() => undefined}
      />,
    );
    expect(
      screen.getByText(
        "Discard 3 files? Their changes are put back the way the team has them.",
      ),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Confirm discard" }),
    ).toHaveFocus();
  });

  it("confirms, cancels, and reads Escape as a cancel", async () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    render(
      <DiscardConfirm
        question="Discard notes/a.md?"
        pending={false}
        problem={null}
        onConfirm={onConfirm}
        onCancel={onCancel}
      />,
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Confirm discard" }),
    );
    expect(onConfirm).toHaveBeenCalledTimes(1);
    await userEvent.click(
      screen.getByRole("button", { name: "Cancel discard" }),
    );
    expect(onCancel).toHaveBeenCalledTimes(1);
    // The strip lives inside a dialog whose own Escape means something else,
    // so this one is answered here and goes no further.
    await userEvent.keyboard("{Escape}");
    expect(onCancel).toHaveBeenCalledTimes(2);
  });

  it("shows a refusal and holds the confirm while the discard is in flight", () => {
    render(
      <DiscardConfirm
        question="Discard notes/a.md?"
        pending
        problem="Changed since you looked."
        onConfirm={() => undefined}
        onCancel={() => undefined}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Changed since you looked.",
    );
    expect(
      screen.getByRole("button", { name: "Confirm discard" }),
    ).toBeDisabled();
  });
});
