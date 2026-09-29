/**
 * The connecting screen: the station dialling in when it starts (M4 C26).
 *
 * A full-screen C64 frame in the device refusal's colours types out, at
 * `TYPE_CPS` characters a second, the dial command with the number, the
 * modem's answer and the name of the place dialled into, while the
 * director plays the dial-in (`audio/modem.ts`). It is done (`onDone`)
 * after `HANDSHAKE_MS`, or at once on any key or click; the host then cuts
 * the sound and hands the station its keys back. A browser's shortcut (a
 * key held with Ctrl, Cmd or Alt, or F1 to F12) is left to the browser:
 * it neither skips the screen nor is cancelled.
 *
 * Its listeners are on `window` in the capture phase, so they run before
 * the session's own Esc listener whatever order they were added in (C9):
 * the host clears `busy` inside `onDone`, and the `preventDefault` on the
 * key taken is what keeps an Esc that skips this screen from also pausing
 * the station behind it.
 *
 * The typing runs on `performance.now()`, the station's own clock, so a
 * wall clock set back or stepped forward neither stalls the lines nor
 * prints them all at once. The screen takes the focus when it shows and
 * gives it back to what had it when it goes. A screen reader hears each
 * line once it is typed out: the typed characters are hidden from it, and
 * a polite live region, mounted empty with the screen, is handed every
 * whole line.
 */

import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { HANDSHAKE_MS } from "../audio/modem";
import { C64_BLUE, C64_LIGHT_BLUE } from "./DeviceRefusal";

/** The screen's accessible name. */
export const CONNECTING_LABEL = "Connecting";

/** How fast the lines are typed out, in characters a second. */
export const TYPE_CPS = 30;

/** A function key: F1 to F12 (a reload, the developer tools, full screen). */
const FUNCTION_KEY = /^F([1-9]|1[0-2])$/;

/**
 * A browser's key, left alone: one held with Ctrl, Cmd or Alt, or a
 * function key. It neither skips the screen nor is cancelled.
 */
function isShortcut(event: KeyboardEvent): boolean {
  return (
    event.ctrlKey ||
    event.metaKey ||
    event.altKey ||
    FUNCTION_KEY.test(event.code)
  );
}

/** The props of `Connecting`. */
export interface ConnectingProps {
  /** The number dialled (`dialNumber`). */
  number: string;
  /** The name of the place dialled into, upper-cased by the host. */
  name: string;
  /** The screen is done: its time ran out, or a key or a click skipped it. */
  onDone: () => void;
}

/** The screen. See the module doc. */
export function Connecting({ number, name, onDone }: ConnectingProps) {
  const lines = [`ATDT ${number}`, "CONNECT 2400", name];
  const total = lines.reduce((sum, line) => sum + line.length, 0);
  const [typed, setTyped] = useState(0);
  const dialog = useRef<HTMLDivElement>(null);
  const doneRef = useRef(onDone);
  useEffect(() => {
    doneRef.current = onDone;
  }, [onDone]);

  // In the commit that shows it, so no key lands before it has the focus.
  useLayoutEffect(() => {
    const before = document.activeElement;
    dialog.current?.focus();
    return () => {
      if (before instanceof HTMLElement && before !== document.body) {
        if (before.isConnected) before.focus();
      }
    };
  }, []);

  useEffect(() => {
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      doneRef.current();
    };
    const onKey = (event: KeyboardEvent) => {
      if (isShortcut(event)) return;
      event.preventDefault();
      finish();
    };
    const onClick = () => {
      finish();
    };
    const started = performance.now();
    const typing = setInterval(() => {
      const count = Math.floor(
        ((performance.now() - started) * TYPE_CPS) / 1000,
      );
      setTyped(Math.min(count, total));
      if (count >= total) clearInterval(typing);
    }, 1000 / TYPE_CPS);
    const timer = setTimeout(finish, HANDSHAKE_MS);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("click", onClick, true);
    return () => {
      done = true;
      clearInterval(typing);
      clearTimeout(timer);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("click", onClick, true);
    };
  }, [total]);

  let left = typed;
  const shown = lines.map((line) => {
    const part = line.slice(0, Math.max(0, left));
    left -= line.length;
    return part;
  });
  const whole = lines.filter((line, i) => shown[i] === line);

  return (
    <div
      ref={dialog}
      role="dialog"
      aria-modal="true"
      aria-label={CONNECTING_LABEL}
      tabIndex={-1}
      className="fixed inset-0 z-50 flex items-center justify-center p-8 font-mono text-lg uppercase outline-none"
      style={{ background: C64_LIGHT_BLUE }}
    >
      <div
        aria-hidden
        className="w-full max-w-2xl p-8"
        style={{ background: C64_BLUE, color: C64_LIGHT_BLUE }}
      >
        {shown.map((part, i) => (
          <p key={i} className="min-h-[1.75rem]">
            {part}
          </p>
        ))}
      </div>
      <div aria-live="polite" className="sr-only">
        {whole.map((line, i) => (
          <p key={i}>{line}</p>
        ))}
      </div>
    </div>
  );
}
