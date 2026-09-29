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
    for (const line of LINES) expect(dialog.textContent).toContain(line);
    expect(onDone).not.toHaveBeenCalled();
  });

  // Mutation caught: the typing timed by `Date.now()` (a wall clock set
  // back stalls the lines); the clock read at the interval's rate instead
  // of the elapsed time.
  it("types by performance.now(), not by the wall clock", () => {
    render(<Connecting number="555-0142" name="ENG" onDone={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: CONNECTING_LABEL });
    // The wall clock steps back an hour, as an NTP step or a wake can.
    vi.setSystemTime(Date.now() - 3_600_000);
    act(() => {
      vi.advanceTimersByTime(
        Math.ceil(((LINES[0]!.length + 1) * 1000) / TYPE_CPS),
      );
    });
    expect(dialog.textContent).toContain("ATDT 555-0142");
  });

  // Mutation caught: the focus left where it was (a screen reader user
  // hears nothing of the screen), or not given back when it goes.
  it("takes the focus when it shows and gives it back when it goes", () => {
    const before = document.createElement("button");
    document.body.append(before);
    before.focus();
    try {
      const view = render(
        <Connecting number="555-0142" name="ENG" onDone={vi.fn()} />,
      );
      expect(document.activeElement).toBe(
        screen.getByRole("dialog", { name: CONNECTING_LABEL }),
      );
      view.unmount();
      expect(document.activeElement).toBe(before);
    } finally {
      before.remove();
    }
  });

  // Mutation caught: no live region (a screen reader hears nothing for up
  // to five seconds), one mounted only with its text, or the typed
  // characters themselves announced one by one.
  it("hands each whole line to a polite live region mounted empty", () => {
    render(<Connecting number="555-0142" name="ENG" onDone={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: CONNECTING_LABEL });
    const live = dialog.querySelectorAll('[aria-live="polite"]');
    expect(live).toHaveLength(1);
    const region = live[0]!;
    expect(region.textContent).toBe("");
    // A line half typed is not in it yet.
    act(() => {
      vi.advanceTimersByTime(Math.ceil((4 * 1000) / TYPE_CPS));
    });
    expect(region.textContent).toBe("");
    act(() => {
      vi.advanceTimersByTime(
        Math.ceil(((LINES[0]!.length + 1) * 1000) / TYPE_CPS),
      );
    });
    expect(region.textContent).toBe("ATDT 555-0142");
    act(() => {
      vi.advanceTimersByTime(
        Math.ceil((LINES.join("").length * 1000) / TYPE_CPS),
      );
    });
    expect([...region.children].map((p) => p.textContent)).toEqual(LINES);
    // The typed characters are hidden from a screen reader.
    for (const p of dialog.querySelectorAll("p")) {
      if (region.contains(p)) continue;
      expect(p.closest("[aria-hidden]")).not.toBeNull();
    }
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

  // Mutation caught: every key taken, browser shortcuts included (F5 or
  // Cmd+R in the first seconds would only skip the screen).
  it("lets browser shortcuts through: neither cancelled nor a skip", () => {
    const onDone = vi.fn();
    render(<Connecting number="555-0142" name="ENG" onDone={onDone} />);
    const shortcuts: KeyboardEventInit[] = [
      { key: "r", code: "KeyR", metaKey: true },
      { key: "l", code: "KeyL", ctrlKey: true },
      { key: "ArrowLeft", code: "ArrowLeft", altKey: true },
      { key: "F5", code: "F5" },
      { key: "F12", code: "F12" },
      { key: "F1", code: "F1" },
    ];
    for (const init of shortcuts) {
      const event = new KeyboardEvent("keydown", {
        ...init,
        bubbles: true,
        cancelable: true,
      });
      act(() => {
        document.body.dispatchEvent(event);
      });
      expect(event.defaultPrevented, init.code).toBe(false);
      expect(onDone, init.code).not.toHaveBeenCalled();
    }
    // Shift is no shortcut: it skips.
    fireEvent.keyDown(document.body, { key: "Shift", code: "ShiftLeft" });
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
    // The focus it takes queues jsdom's own zero-delay selection timer:
    // let that one run, so the count below is the screen's timers alone.
    act(() => {
      vi.advanceTimersByTime(0);
    });
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
