/**
 * The lift overlay: the listing (in the lift's own order), the filter, the
 * selection, the keys, and the note line. The stops are given as a prop, so
 * unlike the level select there is no query to await.
 */
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { LiftStop } from "../world/types";
import { LiftSelect } from "./LiftSelect";
import { LEVEL_ROWS } from "./levels";
import { LIFT_FOOTER, NO_STOPS, NO_SUCH_STOP } from "./lift";

/** One stop, with every field but the label defaulted. */
function stop(label: string, key = false, here = false): LiftStop {
  return { label, to: { kind: "airlock" }, key, here };
}

/** Mounts the overlay over these stops. */
function renderSelect(stops: LiftStop[], note: string | null = null) {
  const onRide = vi.fn<(stop: number) => void>();
  const onClose = vi.fn<() => void>();
  render(
    <LiftSelect stops={stops} note={note} onRide={onRide} onClose={onClose} />,
  );
  const field = screen.getByRole("textbox", { name: "Stop name" });
  return { onRide, onClose, field };
}

/** The labels of the rows shown, top to bottom. */
function rows(): string[] {
  return screen
    .queryAllByRole("option")
    .map((row) => row.firstElementChild?.textContent ?? "");
}

/** The label of the selected row. */
function selected(): string | undefined {
  return screen.getByRole("option", { selected: true }).firstElementChild
    ?.textContent;
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("LiftSelect", () => {
  it("lists the stops in the lift's own order, the first selected, HERE on the current one", () => {
    const { field } = renderSelect([
      stop("Zed"),
      stop("Alpha", false, true),
      stop("Mid"),
    ]);
    expect(screen.getByRole("dialog", { name: "Choose a stop" })).toBeVisible();
    expect(field).toHaveFocus();
    // Not sorted: "Alpha" would come first if it were.
    expect(rows()).toEqual(["Zed", "Alpha", "Mid"]);
    expect(selected()).toBe("Zed");
    const options = screen.getAllByRole("option");
    expect(within(options[1]!).getByText("HERE")).toBeInTheDocument();
    expect(within(options[0]!).queryByText("HERE")).toBeNull();
    expect(screen.getByText("1/3")).toBeInTheDocument();
  });

  it("keeps the footer's double spaces from collapsing", () => {
    renderSelect([stop("a")]);
    const footer = screen.getByText(
      (_, element) => element?.textContent === LIFT_FOOTER,
    );
    expect(footer).toHaveClass("whitespace-pre");
  });

  it("filters by label ignoring case, and says NO SUCH STOP when nothing matches", () => {
    const { field, onRide } = renderSelect([
      stop("Engineering"),
      stop("Bridge"),
      stop("Archive"),
    ]);
    fireEvent.change(field, { target: { value: " brid " } });
    expect(rows()).toEqual(["Bridge"]);

    fireEvent.change(field, { target: { value: "zzz" } });
    expect(screen.getByText(NO_SUCH_STOP)).toBeInTheDocument();
    expect(rows()).toEqual([]);
    expect(screen.getByText("0/0")).toBeInTheDocument();
    fireEvent.keyDown(field, { key: "Enter" });
    expect(onRide).not.toHaveBeenCalled();
  });

  it("says NO STOPS, not NO SUCH STOP, when the lift lists no stops at all", () => {
    // Mutation caught: an empty listing (never a filter's doing) reading as
    // a filter miss, which would leave a reader with no domains believing
    // they typed a bad filter into an empty field.
    renderSelect([]);
    expect(screen.getByText(NO_STOPS)).toBeInTheDocument();
    expect(screen.queryByText(NO_SUCH_STOP)).toBeNull();
    expect(screen.getByText("0/0")).toBeInTheDocument();
  });

  it("moves the selection with the arrows, clamped, resets it on a new filter", () => {
    const { field } = renderSelect([stop("a"), stop("b"), stop("c")]);
    fireEvent.keyDown(field, { key: "ArrowUp" });
    expect(selected()).toBe("a");
    for (let i = 0; i < 5; i++) fireEvent.keyDown(field, { key: "ArrowDown" });
    expect(selected()).toBe("c");
    fireEvent.change(field, { target: { value: "x" } });
    fireEvent.change(field, { target: { value: "" } });
    expect(selected()).toBe("a");
  });

  it("rides on Enter with the stop's index in the unfiltered stops array", () => {
    const { field, onRide } = renderSelect([
      stop("Alpha"),
      stop("Bridge"),
      stop("Archive"),
    ]);
    // Filtering narrows what's shown to "Bridge" and "Archive" (positions 0
    // and 1 in the filtered list), but their real indices are 1 and 2.
    fireEvent.change(field, { target: { value: "r" } });
    expect(rows()).toEqual(["Bridge", "Archive"]);
    fireEvent.keyDown(field, { key: "ArrowDown" });
    fireEvent.keyDown(field, { key: "Enter" });
    expect(onRide).toHaveBeenCalledTimes(1);
    expect(onRide).toHaveBeenCalledWith(2);
  });

  it("rides on a click with the stop's index in the unfiltered stops array, and a mousedown keeps the focus in the field", () => {
    const { field, onRide } = renderSelect([
      stop("Alpha"),
      stop("Bridge"),
      stop("Archive"),
    ]);
    // Filtered to "Bridge" and "Archive": a click on the second shown row
    // (window position 1) must still ride to its real index, 2.
    fireEvent.change(field, { target: { value: "r" } });
    const row = screen.getAllByRole("option")[1]!;
    expect(fireEvent.mouseDown(row)).toBe(false);
    fireEvent.click(row);
    expect(onRide).toHaveBeenCalledWith(2);
  });

  it("closes on Esc, and not on its auto-repeat", () => {
    const { field, onClose } = renderSelect([stop("a")]);
    fireEvent.keyDown(field, { key: "Escape", repeat: true });
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(field, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("leaves Ctrl, Cmd and Alt combinations to the browser", () => {
    const { field, onClose, onRide } = renderSelect([stop("a")]);
    expect(fireEvent.keyDown(field, { key: "Enter", metaKey: true })).toBe(
      true,
    );
    fireEvent.keyDown(field, { key: "Escape", ctrlKey: true });
    expect(onRide).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("swallows the auto-repeat of the key that opened it", () => {
    const { field } = renderSelect([stop("a")]);
    expect(
      fireEvent.keyDown(field, { key: " ", code: "Space", repeat: true }),
    ).toBe(false);
    expect(fireEvent.keyDown(field, { key: "e", code: "KeyE" })).toBe(true);
    expect(
      fireEvent.keyDown(field, { key: "e", code: "KeyE", repeat: true }),
    ).toBe(true);
  });

  it("shows the note under the list", () => {
    renderSelect([stop("a")], "a stop reads sealed");
    const note = screen.getByRole("status");
    expect(note).toHaveTextContent("a stop reads sealed");
    // "Under the list": the listbox must precede the note in document order.
    const listbox = screen.getByRole("listbox");
    expect(
      listbox.compareDocumentPosition(note) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("shows no note line when there is none", () => {
    renderSelect([stop("a")], null);
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("draws a private stop with a key that has the accessible text 'private', before the label", () => {
    renderSelect([stop("Open"), stop("Private", true)]);
    const options = screen.getAllByRole("option");
    expect(within(options[0]!).queryByRole("img")).toBeNull();
    const key = within(options[1]!).getByRole("img", { name: "private" });
    expect(key).toBeInTheDocument();
    // The key comes before the label: its wrapper's first child is the key,
    // and the label text starts only after it.
    const wrapper = options[1]!.firstElementChild;
    expect(wrapper?.firstElementChild).toBe(key);
    expect(wrapper?.textContent).toBe("Private");
  });

  it("a long list: a window of LEVEL_ROWS rows that follows the selection", () => {
    const stops = Array.from({ length: 200 }, (_, i) =>
      stop(`d${String(i).padStart(3, "0")}`),
    );
    const { field, onRide } = renderSelect(stops);
    expect(rows()).toHaveLength(LEVEL_ROWS);

    for (let i = 0; i < 150; i++)
      fireEvent.keyDown(field, { key: "ArrowDown" });
    expect(selected()).toBe("d150");
    expect(rows()).toHaveLength(LEVEL_ROWS);
    expect(screen.getByText("151/200")).toBeInTheDocument();
    fireEvent.keyDown(field, { key: "Enter" });
    expect(onRide).toHaveBeenLastCalledWith(150);

    for (let i = 0; i < 100; i++)
      fireEvent.keyDown(field, { key: "ArrowDown" });
    expect(selected()).toBe("d199");
    expect(rows().at(-1)).toBe("d199");
    expect(rows()).toHaveLength(LEVEL_ROWS);
    expect(screen.getByText("200/200")).toBeInTheDocument();
  });
});
