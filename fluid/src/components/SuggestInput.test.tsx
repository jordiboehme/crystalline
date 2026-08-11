/**
 * The suggesting input, held to the two promises it makes at once: it shows
 * the vocabulary without anybody having to remember it, and it never turns
 * that vocabulary into a rule. Every test here is one half of that pair.
 */

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { FormEvent } from "react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";

import type { Suggestion } from "./SuggestInput";
import { SuggestInput } from "./SuggestInput";

const SUGGESTIONS: Suggestion[] = [
  { name: "stable", gloss: "holds now, and the default" },
  { name: "draft", gloss: "still being written" },
  { name: "superseded", gloss: "replaced by a newer engram" },
];

/**
 * The wiring both call sites have: a label beside the field, the value held by
 * whoever owns it, and somewhere else to put the focus.
 */
function Harness({
  onCommit,
  initial = "",
  suggestions = SUGGESTIONS,
}: {
  onCommit?: (next: string) => void;
  initial?: string;
  suggestions?: Suggestion[];
}) {
  const [value, setValue] = useState(initial);
  return (
    <>
      <label htmlFor="status">Status</label>
      <SuggestInput
        id="status"
        label="Status"
        value={value}
        suggestions={suggestions}
        onChange={setValue}
        {...(onCommit ? { onCommit } : {})}
      />
      <button type="button">Elsewhere</button>
    </>
  );
}

/** The field, by the name a reader and a screen reader both know it by. */
function field(): HTMLElement {
  return screen.getByLabelText("Status");
}

describe("the suggesting input", () => {
  it("opens on focus and shows every recommended value with its gloss", async () => {
    render(<Harness />);
    expect(field()).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();

    await userEvent.click(field());

    expect(field()).toHaveAttribute("aria-expanded", "true");
    const options = screen.getAllByRole("option");
    expect(options.map((option) => option.textContent)).toEqual([
      "stableholds now, and the default",
      "draftstill being written",
      "supersededreplaced by a newer engram",
    ]);
  });

  it("shows the whole list beside a value that is already written", async () => {
    // The reason this control exists: opening the field on `stable` must not
    // narrow the vocabulary to the one word already in it.
    render(<Harness initial="stable" />);
    await userEvent.click(field());
    expect(screen.getAllByRole("option")).toHaveLength(3);
  });

  it("filters to what has been typed", async () => {
    render(<Harness />);
    await userEvent.click(field());
    await userEvent.type(field(), "sup");
    expect(
      screen.getAllByRole("option").map((option) => option.dataset.value),
    ).toEqual(["superseded"]);
  });

  it("fills on Enter once a row has been walked to", async () => {
    const onCommit = vi.fn();
    render(<Harness onCommit={onCommit} />);
    await userEvent.click(field());
    await userEvent.type(field(), "dra");
    await userEvent.keyboard("{ArrowDown}");
    await userEvent.keyboard("{Enter}");

    expect(field()).toHaveValue("draft");
    expect(onCommit).toHaveBeenCalledWith("draft");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("keeps the typed text when Enter finds no row walked to", async () => {
    // The whole point of the field being free form: `e` is inside `stable`,
    // `superseded` and every other word on offer, and Enter must still leave
    // `e` in the field. Nothing is active until an arrow key says so.
    const onCommit = vi.fn();
    render(<Harness onCommit={onCommit} />);
    await userEvent.click(field());
    await userEvent.type(field(), "e");
    expect(field()).not.toHaveAttribute("aria-activedescendant");
    expect(
      screen
        .getAllByRole("option")
        .filter((option) => option.ariaSelected === "true"),
    ).toEqual([]);

    await userEvent.keyboard("{Enter}");
    expect(field()).toHaveValue("e");
    expect(onCommit).toHaveBeenCalledWith("e");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("fills on a click", async () => {
    const onCommit = vi.fn();
    render(<Harness onCommit={onCommit} />);
    await userEvent.click(field());
    await userEvent.click(screen.getByRole("option", { name: /superseded/ }));

    expect(field()).toHaveValue("superseded");
    expect(onCommit).toHaveBeenCalledWith("superseded");
  });

  it("keeps a value nobody recommends through a blur", async () => {
    const onCommit = vi.fn();
    render(<Harness onCommit={onCommit} />);
    await userEvent.click(field());
    await userEvent.type(field(), "brewing");
    await userEvent.tab();

    expect(field()).toHaveValue("brewing");
    expect(onCommit).toHaveBeenCalledWith("brewing");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("closes on Escape with what was typed still in the field", async () => {
    const onCommit = vi.fn();
    render(<Harness onCommit={onCommit} />);
    await userEvent.click(field());
    await userEvent.type(field(), "brew");
    await userEvent.keyboard("{Escape}");

    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(field()).toHaveValue("brew");
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("walks the list with the arrow keys, saying which row is active", async () => {
    render(<Harness />);
    await userEvent.click(field());
    const options = screen.getAllByRole("option");
    // An open list, and nothing chosen in it yet.
    expect(field()).not.toHaveAttribute("aria-activedescendant");
    expect(options[0]).toHaveAttribute("aria-selected", "false");

    await userEvent.keyboard("{ArrowDown}");
    expect(field()).toHaveAttribute("aria-activedescendant", options[0]?.id);
    expect(screen.getAllByRole("option")[0]).toHaveAttribute(
      "aria-selected",
      "true",
    );

    await userEvent.keyboard("{ArrowDown}");
    expect(field()).toHaveAttribute("aria-activedescendant", options[1]?.id);

    await userEvent.keyboard("{Enter}");
    expect(field()).toHaveValue("draft");
  });

  it("reaches the last row with an arrow up from no row", async () => {
    render(<Harness />);
    await userEvent.click(field());
    await userEvent.keyboard("{ArrowUp}");
    const options = screen.getAllByRole("option");
    expect(field()).toHaveAttribute("aria-activedescendant", options[2]?.id);
  });

  it("lets one Enter submit the form around it, carrying the typed text", async () => {
    // The choice, pinned: with no row walked to, Enter is the form's. It was
    // the form's when these fields were plain inputs with a datalist, one
    // press submitted then, and one press submits now - no dead keystroke and
    // nothing substituted on the way.
    const onSubmit = vi.fn((event: FormEvent) => {
      event.preventDefault();
    });
    render(
      <form onSubmit={onSubmit}>
        <Harness />
        <button type="submit">Create</button>
      </form>,
    );
    await userEvent.click(field());
    await userEvent.type(field(), "brewing");
    await userEvent.keyboard("{Enter}");

    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(field()).toHaveValue("brewing");
  });

  it("keeps Enter to itself while a row is walked to", async () => {
    const onSubmit = vi.fn((event: FormEvent) => {
      event.preventDefault();
    });
    render(
      <form onSubmit={onSubmit}>
        <Harness />
        <button type="submit">Create</button>
      </form>,
    );
    await userEvent.click(field());
    await userEvent.keyboard("{ArrowDown}");
    await userEvent.keyboard("{Enter}");

    expect(field()).toHaveValue("stable");
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("gives the list a name of its own, not the field's", async () => {
    render(<Harness />);
    await userEvent.click(field());
    expect(
      screen.getByRole("listbox", { name: "Status suggestions" }),
    ).toBeInTheDocument();
    // And the field is still the one thing called "Status".
    expect(screen.getByLabelText("Status").tagName).toBe("INPUT");
  });

  it("opens on ArrowDown from a closed field", async () => {
    render(<Harness />);
    field().focus();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();

    await userEvent.keyboard("{ArrowDown}");
    expect(screen.getAllByRole("option")).toHaveLength(3);
  });

  it("says how many engrams already carry a value when it is told", async () => {
    render(
      <Harness
        suggestions={[
          { name: "stable", gloss: "holds now, and the default", count: 12 },
        ]}
      />,
    );
    await userEvent.click(field());
    expect(screen.getByRole("option", { name: /stable/ })).toHaveTextContent(
      "12",
    );
  });
});
