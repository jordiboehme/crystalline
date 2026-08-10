/**
 * The room's conflict view: both sides on screen, and no way out that throws
 * either of them away by accident.
 */

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { CollabConflictDialog } from "./CollabConflictDialog";

describe("CollabConflictDialog", () => {
  it("shows both sides verbatim and resolves mine", async () => {
    const onResolve = vi.fn();
    render(
      <CollabConflictDialog
        conflict={{
          kind: "edit",
          theirs: "THEIR text",
          detail: "an agent rewrote this engram",
        }}
        mine="MY text"
        onResolve={onResolve}
        onClose={() => undefined}
      />,
    );
    expect(screen.getByText("an agent rewrote this engram")).toBeVisible();
    expect(screen.getByText("THEIR text")).toBeVisible();
    expect(screen.getByText("MY text")).toBeVisible();
    await userEvent.click(
      screen.getByRole("button", { name: "Keep the session text" }),
    );
    expect(onResolve).toHaveBeenCalledWith("mine");
  });

  it("offers the deletion wording when theirs is gone", async () => {
    const onResolve = vi.fn();
    render(
      <CollabConflictDialog
        conflict={{
          kind: "deleted",
          theirs: null,
          detail: "the file was deleted outside this session",
        }}
        mine="MY text"
        onResolve={onResolve}
        onClose={() => undefined}
      />,
    );
    expect(
      screen.getByText("This engram's file was deleted outside the session"),
    ).toBeVisible();
    await userEvent.click(
      screen.getByRole("button", { name: "Accept the deletion" }),
    );
    expect(onResolve).toHaveBeenCalledWith("theirs");
  });

  it("closing keeps editing without resolving", async () => {
    const onClose = vi.fn();
    const onResolve = vi.fn();
    render(
      <CollabConflictDialog
        conflict={{ kind: "edit", theirs: "T", detail: "d" }}
        mine="M"
        onResolve={onResolve}
        onClose={onClose}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    expect(onClose).toHaveBeenCalled();
    expect(onResolve).not.toHaveBeenCalled();
  });

  it("keeps the session text on screen when the deletion is what is offered", () => {
    render(
      <CollabConflictDialog
        conflict={{ kind: "deleted", theirs: null, detail: "d" }}
        mine="MY unsaved paragraph"
        onResolve={() => undefined}
        onClose={() => undefined}
      />,
    );
    // Neither exit may be taken blind: accepting the deletion gives up text
    // that has to be readable while the choice is made.
    expect(screen.getByText("MY unsaved paragraph")).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Restore with the session text" }),
    ).toBeVisible();
  });

  it("says so rather than showing an empty pane when their text is unknown", () => {
    // The mid-conflict joiner's shape: the room is in conflict, but the
    // broadcast that carried their bytes predates this tab's subscription.
    render(
      <CollabConflictDialog
        conflict={{ kind: "edit", theirs: null, detail: "d" }}
        mine="MY text"
        onResolve={() => undefined}
        onClose={() => undefined}
      />,
    );
    expect(
      screen.getByText("The file's text could not be read from here."),
    ).toBeVisible();
    expect(screen.getByText("MY text")).toBeVisible();
  });
});
