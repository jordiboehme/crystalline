/**
 * The pause screen: what it shows, when Esc leaves, and why CONT waits.
 *
 * The screen reads its clock through its `now` prop, so the tests hand it
 * one they set by hand, and fake timers run the timeout that makes CONT
 * usable once `RELOCK_DELAY_MS` has passed since the lock ended.
 */

import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { RELOCK_DELAY_MS } from "../core/input";
import { PAUSE_ESC_GUARD_MS } from "../session";
import { PAUSE_LEGEND } from "./keys";
import {
  CONT_LABEL,
  PAUSE_LABEL,
  PauseScreen,
  RUN_STOP_LABEL,
} from "./PauseScreen";

let t = 0;
const now = () => t;

/** Mounts the screen at `t`, the lock having ended at `lockEndedAt`. */
function renderPause(lockEndedAt: number, where: string | null = "ALPHA") {
  const onContinue = vi.fn<() => void>();
  const onLeave = vi.fn<() => void>();
  const view = render(
    <PauseScreen
      where={where}
      lockEndedAt={lockEndedAt}
      onContinue={onContinue}
      onLeave={onLeave}
      now={now}
    />,
  );
  return { onContinue, onLeave, view };
}

/** A keydown on the window, as the screen hears it. */
function press(key: string, repeat = false) {
  fireEvent.keyDown(window, { key, repeat });
}

/** Moves the clock and the timers on by `ms`. */
function pass(ms: number) {
  t += ms;
  act(() => {
    vi.advanceTimersByTime(ms);
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  t = 10_000;
});

afterEach(() => {
  vi.useRealTimers();
});

describe("PauseScreen", () => {
  it("shows where the station broke, READY. once CONT is usable, and the legend", () => {
    renderPause(-Infinity, "Alpha Note");
    const dialog = screen.getByRole("dialog", { name: PAUSE_LABEL });
    expect(PAUSE_LABEL).toBe("Paused");
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(screen.getByText("BREAK IN ALPHA NOTE")).toBeInTheDocument();
    expect(screen.getByText("READY.")).toBeInTheDocument();
    expect(screen.getByText(PAUSE_LEGEND)).toBeInTheDocument();
    const cont = screen.getByRole("button", { name: CONT_LABEL });
    expect(cont).toBeEnabled();
    expect(cont).toHaveFocus();
    expect(
      screen.getByRole("button", { name: RUN_STOP_LABEL }),
    ).toBeInTheDocument();
    expect([CONT_LABEL, RUN_STOP_LABEL]).toEqual(["CONT", "RUN/STOP (ESC)"]);
  });

  it("says only BREAK where the room has no label", () => {
    renderPause(-Infinity, null);
    expect(screen.getByText("BREAK")).toBeInTheDocument();
  });

  it("leaves only on a fresh Esc after the guard", () => {
    // Mutation caught: the guard dropped (the Esc at 50 ms leaves), the
    // repeat not checked (the held Esc leaves).
    const { onLeave, onContinue } = renderPause(-Infinity);
    pass(50);
    press("Escape");
    expect(onLeave).not.toHaveBeenCalled();
    pass(PAUSE_ESC_GUARD_MS);
    press("Escape", true);
    expect(onLeave).not.toHaveBeenCalled();
    press("Escape");
    expect(onLeave).toHaveBeenCalledTimes(1);
    expect(onContinue).not.toHaveBeenCalled();
  });

  it("leaves on RUN/STOP at once, the lock's wait or not", () => {
    const { onLeave } = renderPause(t);
    fireEvent.click(screen.getByRole("button", { name: RUN_STOP_LABEL }));
    expect(onLeave).toHaveBeenCalledTimes(1);
  });

  it("holds CONT, Enter, Space and the backdrop until the lock can be asked for again (F17)", () => {
    // Mutation caught: the wait dropped (any of the four continues at
    // once, and the browser refuses the lock it asks for).
    const { onContinue, onLeave } = renderPause(t);
    const dialog = screen.getByRole("dialog", { name: PAUSE_LABEL });
    const cont = screen.getByRole("button", { name: CONT_LABEL });
    expect(cont).toBeDisabled();
    expect(screen.queryByText("READY.")).toBeNull();
    fireEvent.click(cont);
    press("Enter");
    press(" ");
    fireEvent.click(dialog);
    pass(RELOCK_DELAY_MS - 100);
    press("Enter");
    expect(onContinue).not.toHaveBeenCalled();
    expect(screen.queryByText("READY.")).toBeNull();

    pass(100);
    expect(screen.getByText("READY.")).toBeInTheDocument();
    expect(cont).toBeEnabled();
    expect(cont).toHaveFocus();
    fireEvent.click(cont);
    expect(onContinue).toHaveBeenCalledTimes(1);
    press("Enter");
    expect(onContinue).toHaveBeenCalledTimes(2);
    press(" ");
    expect(onContinue).toHaveBeenCalledTimes(3);
    fireEvent.click(dialog);
    expect(onContinue).toHaveBeenCalledTimes(4);
    // A click on the screen's text is not a click on the backdrop.
    fireEvent.click(screen.getByText(PAUSE_LEGEND));
    expect(onContinue).toHaveBeenCalledTimes(4);
    expect(onLeave).not.toHaveBeenCalled();
  });

  it("ignores a held Enter or Space and leaves a focused button's Enter to the button", () => {
    // Mutation caught: the repeat not checked on Enter and Space (a held
    // key would continue), the focused-button return dropped (Enter on a
    // focused RUN/STOP would continue as well as leave).
    const { onContinue } = renderPause(-Infinity);
    press("Enter", true);
    press(" ", true);
    expect(onContinue).not.toHaveBeenCalled();
    const runStop = screen.getByRole("button", { name: RUN_STOP_LABEL });
    runStop.focus();
    fireEvent.keyDown(runStop, { key: "Enter" });
    fireEvent.keyDown(runStop, { key: " " });
    expect(onContinue).not.toHaveBeenCalled();
    press("Enter");
    expect(onContinue).toHaveBeenCalledTimes(1);
  });

  it("leaves on Esc while CONT still waits", () => {
    const { onLeave } = renderPause(t);
    pass(PAUSE_ESC_GUARD_MS);
    press("Escape");
    expect(onLeave).toHaveBeenCalledTimes(1);
  });
});
