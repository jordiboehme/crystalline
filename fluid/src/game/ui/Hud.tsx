/**
 * The heads-up display: everything drawn over the station besides the room.
 *
 * The prompt for what the player faces sits at the bottom centre, the
 * status line (room, look, condition, mouse hint) at the bottom left, the
 * frame time at the top right beside an optional key legend, and a notice
 * (`ACCESS DENIED`, `LOOK INVERTED`) in the middle. Over all of them, while
 * travelling, the connector.
 *
 * `useHud` (in `useHud.ts`) makes the `HudSink` the session writes to and
 * the callback refs this component binds its text lines with: those lines
 * are written straight into the DOM, never rendered through React state.
 * The connector is React state the host passes in; the host mounts the CRT
 * reader itself.
 */

import { Connector, type ConnectorState } from "./Connector";
import type { HudView } from "./useHud";

/** The props of `Hud`. */
export interface HudProps {
  view: HudView;
  connector: ConnectorState;
  /** A key legend for the top left, or nothing. */
  legend?: string;
}

/**
 * The HUD's elements over a full-screen canvas: the text lines the sink
 * writes and the connector. None of it takes the mouse, so a click reaches
 * the canvas and asks for pointer lock.
 */
export function Hud({ view, connector, legend }: HudProps) {
  const { prompt, status, frame, notice } = view;
  const line =
    "pointer-events-none font-mono text-xs text-white/85 [text-shadow:0_1px_2px_black]";
  return (
    <>
      <div
        className={`absolute inset-x-0 top-0 flex justify-between gap-4 p-3 ${line}`}
      >
        <span>{legend ?? ""}</span>
        <span ref={frame} />
      </div>
      <div className={`absolute inset-x-0 bottom-0 p-3 ${line}`}>
        <span ref={status} />
      </div>
      <div
        ref={prompt}
        hidden
        className="pointer-events-none absolute inset-x-0 bottom-12 text-center font-mono text-base tracking-wider text-white [text-shadow:0_1px_3px_black]"
      />
      <div
        ref={notice}
        hidden
        className="pointer-events-none absolute inset-0 flex items-center justify-center font-mono text-lg text-white [text-shadow:0_1px_3px_black]"
      />
      <Connector {...connector} />
    </>
  );
}
