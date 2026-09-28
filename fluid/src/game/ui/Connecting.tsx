/**
 * The connecting screen: the station dialling in when it starts (M4 C26).
 *
 * A full-screen C64 frame in the device refusal's colours types out, at
 * `TYPE_CPS` characters a second, the dial command with the number, the
 * modem's answer and the name of the place dialled into, while the
 * director plays the dial-in (`audio/modem.ts`). It is done (`onDone`)
 * after `HANDSHAKE_MS`, or at once on any key or click; the host then cuts
 * the sound and hands the station its keys back.
 *
 * Its listeners are on `window` in the capture phase, so they run before
 * the session's own Esc listener whatever order they were added in (C9):
 * the host clears `busy` inside `onDone`, and the `preventDefault` on the
 * key taken is what keeps an Esc that skips this screen from also pausing
 * the station behind it.
 */

import { useEffect, useRef, useState } from "react";

import { HANDSHAKE_MS } from "../audio/modem";
import { C64_BLUE, C64_LIGHT_BLUE } from "./DeviceRefusal";

/** The screen's accessible name. */
export const CONNECTING_LABEL = "Connecting";

/** How fast the lines are typed out, in characters a second. */
export const TYPE_CPS = 30;

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
  const doneRef = useRef(onDone);
  useEffect(() => {
    doneRef.current = onDone;
  }, [onDone]);

  useEffect(() => {
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      doneRef.current();
    };
    const onKey = (event: KeyboardEvent) => {
      event.preventDefault();
      finish();
    };
    const onClick = () => {
      finish();
    };
    const started = Date.now();
    const typing = setInterval(() => {
      const count = Math.floor(((Date.now() - started) * TYPE_CPS) / 1000);
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

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={CONNECTING_LABEL}
      className="fixed inset-0 z-50 flex items-center justify-center p-8 font-mono text-lg uppercase"
      style={{ background: C64_LIGHT_BLUE }}
    >
      <div
        className="w-full max-w-2xl p-8"
        style={{ background: C64_BLUE, color: C64_LIGHT_BLUE }}
      >
        {shown.map((part, i) => (
          <p key={i} className="min-h-[1.75rem]">
            {part}
          </p>
        ))}
      </div>
    </div>
  );
}
