/**
 * The two-step confirm itself, rendered on its own: it reaches no API and
 * needs no screen around it, so it is tested directly rather than through a
 * card that happens to use it.
 *
 * What is pinned here is the third step the domain page asked for - a
 * confirmation that will not fire until the name of the thing being destroyed
 * has been typed exactly - and the two ways of owning the confirmation, the
 * control's own and a parent's.
 */

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { DestructiveAction } from "./DestructiveAction";

describe("the destructive action", () => {
  it("keeps the confirm press inert until the exact name is typed", async () => {
    const onConfirm = vi.fn();
    render(
      <DestructiveAction
        label="Unregister domain"
        confirmLabel="Confirm unregister"
        pending={false}
        requireMatch="eng"
        onConfirm={onConfirm}
      />,
    );

    await userEvent.click(
      screen.getByRole("button", { name: "Unregister domain" }),
    );
    const confirm = screen.getByRole("button", { name: "Confirm unregister" });
    // Armed and refusing: the field is empty, so the press says so and does
    // nothing when it is pressed anyway.
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(confirm);
    expect(onConfirm).not.toHaveBeenCalled();

    const field = screen.getByLabelText("Type eng to confirm");
    await userEvent.type(field, "en");
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(confirm);
    expect(onConfirm).not.toHaveBeenCalled();

    await userEvent.type(field, "g");
    expect(confirm).toHaveAttribute("aria-disabled", "false");
    await userEvent.click(confirm);
    expect(onConfirm).toHaveBeenCalledWith("eng");
  });

  it("treats a near miss as a miss, case and all", async () => {
    const onConfirm = vi.fn();
    render(
      <DestructiveAction
        label="Make private"
        confirmLabel="Confirm make private"
        pending={false}
        requireMatch="eng"
        onConfirm={onConfirm}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: "Make private" }));
    await userEvent.type(screen.getByLabelText("Type eng to confirm"), "ENG");

    // The name of the domain, not a name that looks like it: no case folding
    // and no trimming, because the point of typing it is to have read it.
    const confirm = screen.getByRole("button", {
      name: "Confirm make private",
    });
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(confirm);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("clears the typed name and hands focus back on Escape and on Keep", async () => {
    render(
      <DestructiveAction
        label="Unregister domain"
        confirmLabel="Confirm unregister"
        pending={false}
        requireMatch="eng"
        onConfirm={vi.fn()}
      />,
    );
    const trigger = screen.getByRole("button", { name: "Unregister domain" });

    await userEvent.click(trigger);
    await userEvent.type(screen.getByLabelText("Type eng to confirm"), "eng");
    await userEvent.keyboard("{Escape}");
    expect(
      screen.queryByRole("button", { name: "Confirm unregister" }),
    ).toBeNull();
    expect(document.activeElement).toBe(trigger);

    // Nothing carried over: arming it again starts from an empty field, so a
    // confirmation is never one press away from a name typed minutes ago.
    await userEvent.click(trigger);
    expect(screen.getByLabelText("Type eng to confirm")).toHaveValue("");

    await userEvent.type(screen.getByLabelText("Type eng to confirm"), "eng");
    await userEvent.click(screen.getByRole("button", { name: "Keep" }));
    expect(document.activeElement).toBe(trigger);
    await userEvent.click(trigger);
    expect(screen.getByLabelText("Type eng to confirm")).toHaveValue("");
  });

  it("lets a parent own the confirmation, for a route that arms it from elsewhere", async () => {
    const onConfirmingChange = vi.fn();
    const { rerender } = render(
      <DestructiveAction
        label="Unregister domain"
        confirmLabel="Confirm unregister"
        pending={false}
        requireMatch="eng"
        confirming={false}
        onConfirmingChange={onConfirmingChange}
        onConfirm={vi.fn()}
      />,
    );

    // The parent's flag, not the press, is what opens the second step: this
    // is how the command palette's own row asks the same question.
    expect(
      screen.queryByRole("button", { name: "Confirm unregister" }),
    ).toBeNull();
    rerender(
      <DestructiveAction
        label="Unregister domain"
        confirmLabel="Confirm unregister"
        pending={false}
        requireMatch="eng"
        confirming
        onConfirmingChange={onConfirmingChange}
        onConfirm={vi.fn()}
      />,
    );
    expect(
      screen.getByRole("button", { name: "Confirm unregister" }),
    ).toBeVisible();

    await userEvent.click(screen.getByRole("button", { name: "Keep" }));
    expect(onConfirmingChange).toHaveBeenCalledWith(false);
  });

  it("takes the focus back when the confirmation collapses under it", () => {
    const props = {
      label: "Unregister domain",
      confirmLabel: "Confirm unregister",
      pending: false,
      requireMatch: "eng",
      onConfirmingChange: vi.fn(),
      onConfirm: vi.fn(),
    };
    const { rerender } = render(
      <DestructiveAction {...props} confirming={false} />,
    );
    const trigger = screen.getByRole("button", { name: "Unregister domain" });
    rerender(<DestructiveAction {...props} confirming />);

    // A parent that collapses the confirmation - a refusal from the server,
    // which is exactly where the domain page's unregister lands - cannot
    // reach this control's trigger, and the focus would otherwise drop to the
    // document body with the buttons that held it.
    rerender(<DestructiveAction {...props} confirming={false} />);

    expect(document.activeElement).toBe(trigger);
  });

  it("gives the confirmation up when the reader moves to another control", async () => {
    render(
      <>
        <DestructiveAction
          label="Unregister domain"
          confirmLabel="Confirm unregister"
          pending={false}
          onConfirm={vi.fn()}
        />
        <button type="button">Elsewhere</button>
      </>,
    );

    await userEvent.click(
      screen.getByRole("button", { name: "Unregister domain" }),
    );
    await userEvent.click(screen.getByRole("button", { name: "Elsewhere" }));

    // Focus landed on something the reader chose, so the question is over:
    // this is the second press being abandoned rather than skipped.
    expect(
      screen.queryByRole("button", { name: "Confirm unregister" }),
    ).toBeNull();
  });

  it("keeps it when focus is parked on something out of the tab order", async () => {
    render(
      <>
        <DestructiveAction
          label="Unregister domain"
          confirmLabel="Confirm unregister"
          pending={false}
          onConfirm={vi.fn()}
        />
        <div tabIndex={-1} data-testid="container" />
      </>,
    );

    await userEvent.click(
      screen.getByRole("button", { name: "Unregister domain" }),
    );
    await userEvent.click(screen.getByTestId("container"));

    // A destination nothing can tab to is not a reader moving on: it is what
    // a dialog closing behind this one does on its way out, which is exactly
    // how the command palette leaves the row that armed this. Collapsing
    // there would eat the confirmation the palette route just opened.
    expect(
      screen.getByRole("button", { name: "Confirm unregister" }),
    ).toBeVisible();
  });
});
