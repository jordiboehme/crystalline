import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { HANDSHAKE_MS } from "../audio/modem";
import { CONNECTING_LABEL, Connecting, TYPE_CPS } from "./Connecting";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

const LINES = ["ATDT 555-0142", "CONNECT 2400", "ENG"];

describe("Connecting", () => {
  it("types its three lines out at TYPE_CPS", () => {
    const onDone = vi.fn();
    render(<Connecting number="555-0142" name="ENG" onDone={onDone} />);
    const dialog = screen.getByRole("dialog", { name: CONNECTING_LABEL });
    expect(dialog.textContent).not.toContain("ATDT 555-0142");
    const chars = LINES.join("").length;
    // Half way: the dial line is out, the name is not.
    act(() => {
      vi.advanceTimersByTime(
        Math.ceil(((LINES[0]!.length + 1) * 1000) / TYPE_CPS),
      );
    });
    expect(dialog.textContent).toContain("ATDT 555-0142");
    expect(dialog.textContent).not.toContain("CONNECT 2400");
    act(() => {
      vi.advanceTimersByTime(Math.ceil((chars * 1000) / TYPE_CPS));
    });
    for (const line of LINES)
      expect(screen.getByText(line)).toBeInTheDocument();
    expect(onDone).not.toHaveBeenCalled();
  });

  // Mutation caught: the `preventDefault` dropped (an Esc that skips the
  // screen would also pause the station behind it), or `onDone` called
  // for every key.
  it("skips on any key, once, and cancels the key it took", () => {
    const onDone = vi.fn();
    render(<Connecting number="555-0142" name="ENG" onDone={onDone} />);
    const esc = new KeyboardEvent("keydown", {
      key: "Escape",
      code: "Escape",
      bubbles: true,
      cancelable: true,
    });
    act(() => {
      document.body.dispatchEvent(esc);
    });
    expect(onDone).toHaveBeenCalledTimes(1);
    expect(esc.defaultPrevented).toBe(true);
    fireEvent.keyDown(document.body, { key: "a", code: "KeyA" });
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it("skips on a click", () => {
    const onDone = vi.fn();
    render(<Connecting number="555-0142" name="ENG" onDone={onDone} />);
    fireEvent.click(screen.getByRole("dialog", { name: CONNECTING_LABEL }));
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  // Mutation caught: no timer (the screen stays until a key), or one left
  // running after the unmount (a late `onDone` on a gone screen).
  it("is done after HANDSHAKE_MS and stops its timers on unmount", () => {
    const onDone = vi.fn();
    const view = render(
      <Connecting number="555-0142" name="ENG" onDone={onDone} />,
    );
    act(() => {
      vi.advanceTimersByTime(HANDSHAKE_MS - 1);
    });
    expect(onDone).not.toHaveBeenCalled();
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(onDone).toHaveBeenCalledTimes(1);
    view.unmount();

    const late = vi.fn();
    const second = render(
      <Connecting number="555-0142" name="ENG" onDone={late} />,
    );
    second.unmount();
    expect(vi.getTimerCount()).toBe(0);
    act(() => {
      vi.advanceTimersByTime(HANDSHAKE_MS);
    });
    expect(late).not.toHaveBeenCalled();
    fireEvent.keyDown(document.body, { key: "a", code: "KeyA" });
    expect(late).not.toHaveBeenCalled();
  });
});
