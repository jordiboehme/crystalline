/**
 * The pause screen: the C64's answer to RUN/STOP, over the still station.
 *
 * The session pauses on a lost lock or an unlocked Esc and says so through
 * `onPause` (M4 C6); the host mounts this screen while it is paused. It
 * says where the station stopped (`BREAK IN <room>`, or `BREAK` alone where
 * the room has no label, the console room), then `READY.`, two buttons and
 * the game's keys, in the device refusal's colours.
 *
 * `CONT`, Enter, Space or a click on the border around the screen continues
 * (`onContinue`, which asks for the lock inside that gesture). The browser
 * refuses a new lock for `RELOCK_DELAY_MS` after one ended, so until then
 * all four are held back: `CONT` is disabled and `READY.` not yet printed,
 * and both appear together once it would work (F17). `RUN/STOP (ESC)` or
 * a fresh Esc leaves (`onLeave`) at any time, but an Esc only
 * `PAUSE_ESC_GUARD_MS` after the screen opened, so the Esc that ended the
 * lock, when a browser delivers it too, does not also leave (C7).
 *
 * Its keys are read on `window`, as the other overlays read theirs, and the
 * ones it takes are cancelled. Enter or Space on a focused button is left to
 * the button itself, so RUN/STOP focused and Enter pressed leaves rather
 * than continues.
 */

import { useEffect, useRef, useState } from "react";

import { RELOCK_DELAY_MS } from "../core/input";
import { PAUSE_ESC_GUARD_MS } from "../session";
import { C64_BLUE, C64_LIGHT_BLUE } from "./DeviceRefusal";
import { PAUSE_LEGEND } from "./keys";

/** The pause screen's accessible name. */
export const PAUSE_LABEL = "Paused";
/** The button that continues. */
export const CONT_LABEL = "CONT";
/** The button that leaves the station. */
export const RUN_STOP_LABEL = "RUN/STOP (ESC)";

/** The props of `PauseScreen`. */
export interface PauseScreenProps {
  /** The room's label, as the status line leads with it, or null for none. */
  where: string | null;
  /** When the pointer lock last ended, on `now`'s clock. */
  lockEndedAt: number;
  /** Continue: the host resumes the session inside this event. */
  onContinue: () => void;
  /** Leave the station for the Fluid page of the room. */
  onLeave: () => void;
  /** The clock; `performance.now`, which the lock's end is stamped on. */
  now?: () => number;
}

const performanceNow = () => performance.now();

/** The screen. See the module doc. */
export function PauseScreen({
  where,
  lockEndedAt,
  onContinue,
  onLeave,
  now = performanceNow,
}: PauseScreenProps) {
  const [openedAt] = useState(now);
  const readyAt = lockEndedAt + RELOCK_DELAY_MS;
  const [ready, setReady] = useState(() => now() >= readyAt);
  const dialog = useRef<HTMLDivElement>(null);
  const cont = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (ready) return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const check = () => {
      const left = readyAt - now();
      if (left <= 0) setReady(true);
      else timer = setTimeout(check, left);
    };
    check();
    return () => {
      if (timer !== null) clearTimeout(timer);
    };
  }, [ready, readyAt, now]);

  // CONT takes the focus once it can act; until then the screen holds it,
  // so no key reaches whatever had it before.
  useEffect(() => {
    if (ready) cont.current?.focus();
    else dialog.current?.focus();
  }, [ready]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.ctrlKey || event.metaKey || event.altKey) return;
      switch (event.key) {
        case "Escape":
          if (!event.repeat && now() - openedAt >= PAUSE_ESC_GUARD_MS)
            onLeave();
          break;
        case "Enter":
        case " ":
          if (
            event.target instanceof HTMLButtonElement &&
            dialog.current?.contains(event.target) === true
          )
            return;
          if (!event.repeat && now() >= readyAt) onContinue();
          break;
        default:
          return;
      }
      event.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
    };
  }, [now, openedAt, readyAt, onContinue, onLeave]);

  const button =
    "uppercase underline decoration-dotted underline-offset-4 hover:bg-[#6c5eb5] hover:text-[#352879] focus:bg-[#6c5eb5] focus:text-[#352879] focus:outline-none disabled:no-underline disabled:opacity-60 disabled:hover:bg-transparent disabled:hover:text-[#6c5eb5]";

  return (
    <div
      ref={dialog}
      role="dialog"
      aria-modal="true"
      aria-label={PAUSE_LABEL}
      tabIndex={-1}
      className="fixed inset-0 z-50 flex items-center justify-center p-8 font-mono text-lg uppercase outline-none"
      style={{ background: C64_LIGHT_BLUE }}
      onClick={(event) => {
        if (event.target === event.currentTarget && now() >= readyAt)
          onContinue();
      }}
    >
      <div
        className="w-full max-w-2xl p-8"
        style={{ background: C64_BLUE, color: C64_LIGHT_BLUE }}
      >
        <p>{where === null ? "BREAK" : `BREAK IN ${where.toUpperCase()}`}</p>
        <p aria-live="polite" className="min-h-[1.75rem]">
          {ready ? "READY." : ""}
        </p>
        <p className="mt-6">
          <button
            ref={cont}
            type="button"
            disabled={!ready}
            className={button}
            onClick={() => {
              if (now() >= readyAt) onContinue();
            }}
          >
            {CONT_LABEL}
          </button>
        </p>
        <p className="mt-2">
          <button type="button" className={button} onClick={onLeave}>
            {RUN_STOP_LABEL}
          </button>
        </p>
        <p className="mt-6 text-xs normal-case tracking-wide">{PAUSE_LEGEND}</p>
      </div>
    </div>
  );
}
